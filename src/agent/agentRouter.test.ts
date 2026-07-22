import { beforeEach, describe, expect, it, vi } from "vitest"

import type { AgentConnection, StopReason } from "./acpConnection"
import { createAgentRouter, fingerprintAgentCommand } from "./agentRouter"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so settings
// persistence runs for real (mirrors dbStore.test.ts / agentStore.test.ts).
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

let promptedCommand: string | undefined
let sessionCounter = 0
let droppedSessions: string[] = []

function lastPromptCommand(): string | undefined {
    return promptedCommand
}

function makeStub(command: string, loadCapable = true): AgentConnection {
    const sessions = new Set<string>()
    return {
        async prepare() {
            /* no-op stub */
        },
        async newSession() {
            const id = `${command}-session-${sessionCounter++}`
            sessions.add(id)
            return { sessionId: id, startupInfo: null }
        },
        async loadSession(id) {
            sessions.add(id)
        },
        async listSessions() {
            return [...sessions].map((id) => ({ id, cwd: "" }))
        },
        async prompt(): Promise<StopReason> {
            promptedCommand = command
            return "end_turn"
        },
        cancel() {
            /* no-op stub */
        },
        async supportsLoadSession() {
            return loadCapable
        },
        async setSessionConfigOption() {
            return [{
                id: `${command}-model`,
                name: "Model",
                category: "model",
                type: "select",
                currentValue: "fast",
                options: [{ value: "fast", name: "Fast" }]
            }]
        },
        async disposePrepared() {
            return sessions.size === 0
        },
        dropSession(sessionId) {
            droppedSessions.push(sessionId)
            sessions.delete(sessionId)
        }
    }
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    promptedCommand = undefined
    sessionCounter = 0
    droppedSessions = []
})

describe("createAgentRouter", () => {
    it("creates a distinct sub-connection per (command, cwd) — fixes P10 cwd binding", async () => {
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)
        await router.newSession("/ws-a", "pi")
        await router.newSession("/ws-b", "pi") // 同 agent、不同 cwd
        expect(factory).toHaveBeenCalledTimes(2)
    })

    it("routes prompt to the codex sub-connection when session was created with codex", async () => {
        const router = createAgentRouter({}, (command) => makeStub(command))
        const s = await router.newSession("/ws", "codex")
        await router.prompt(s.sessionId, [{ type: "text", text: "x" }])
        expect(lastPromptCommand()).toBe("bunx @agentclientprotocol/codex-acp@latest")
        expect(s.agentIdentity).toEqual({
            selectedPreset: "codex",
            commandMode: "latest",
            trustedAgentId: "codex"
        })
    })

    it("uses the persisted custom command when agentId is omitted", async () => {
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({ preset: "custom", command: "uvx my-acp", traceEnabled: false })
        )
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)
        await router.newSession("/ws") // 省略 agentId → 走 settings
        expect(factory).toHaveBeenCalledWith("uvx my-acp", "/ws")
    })

    it("returns a one-way identity for the exact custom command used to create a session", async () => {
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({ preset: "custom", command: "uvx private-acp --token super-secret", traceEnabled: false })
        )
        const router = createAgentRouter({}, (command) => makeStub(command))

        const result = await router.newSession("/ws")

        expect(result.customCommandFingerprint).toMatch(/^sha256:[0-9a-f]{64}$/)
        expect(result.customCommandFingerprint).not.toContain("private-acp")
        expect(result.customCommandFingerprint).not.toContain("super-secret")
        expect(result.agentIdentity).toEqual({
            selectedPreset: "custom",
            commandMode: "custom",
            trustedAgentId: null
        })
    })

    it("uses each curated preset's persisted latest/custom mode without trusting custom", async () => {
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({
                preset: "pi",
                command: "bunx pi-acp@latest",
                traceEnabled: false,
                presetCommands: {
                    pi: { mode: "latest", customCommand: "" },
                    claude: { mode: "latest", customCommand: "" },
                    codex: { mode: "custom", customCommand: "uvx wrapped-codex" }
                }
            })
        )
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)

        const latest = await router.newSession("/ws", "claude")
        const custom = await router.newSession("/ws", "codex")

        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/claude-agent-acp@latest", "/ws")
        expect(factory).toHaveBeenCalledWith("uvx wrapped-codex", "/ws")
        expect(latest.agentIdentity?.trustedAgentId).toBe("claude")
        expect(custom.agentIdentity).toMatchObject({
            selectedPreset: "codex",
            commandMode: "custom",
            trustedAgentId: null
        })
        expect(custom.customCommandFingerprint).toMatch(/^sha256:/)
    })

    it("rejects a restored custom route after command drift without creating a sub-connection", async () => {
        const fingerprintA = await fingerprintAgentCommand("uvx agent-a --token secret-a")
        expect(fingerprintA).toBeDefined()
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({ preset: "custom", command: "uvx agent-b", traceEnabled: false })
        )
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)

        await expect(router.supportsLoadSession?.("/ws", undefined, fingerprintA))
            .rejects.toThrow("custom agent command has changed")
        await expect(router.loadSession("restored-a", "/ws", undefined, fingerprintA))
            .rejects.toThrow("custom agent command has changed")
        expect(factory).not.toHaveBeenCalled()
    })

    it("replays a restored custom session only when the command fingerprint still matches", async () => {
        const command = "uvx agent-a"
        const fingerprint = await fingerprintAgentCommand(command)
        expect(fingerprint).toBeDefined()
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({ preset: "custom", command, traceEnabled: false })
        )
        const factory = vi.fn((resolved: string) => makeStub(resolved))
        const router = createAgentRouter({}, factory)

        await expect(router.supportsLoadSession?.("/ws", undefined, fingerprint)).resolves.toBe(true)
        await expect(router.loadSession("restored-a", "/ws", undefined, fingerprint)).resolves.toBeUndefined()
        expect(factory).toHaveBeenCalledTimes(1)
        expect(factory).toHaveBeenCalledWith(command, "/ws")
    })

    it("reuses the existing sub-connection for the same (agentId, cwd) — factory called once", async () => {
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)
        await router.newSession("/ws", "pi")
        await router.newSession("/ws", "pi") // 同 agent、同 cwd
        expect(factory).toHaveBeenCalledTimes(1)
    })

    it("supportsLoadSession routes to the default-command (pi) sub when there's no known session and no agentId", async () => {
        const router = createAgentRouter({}, (command) => makeStub(command, command === "bunx pi-acp@latest"))

        await expect(router.supportsLoadSession?.("/ws")).resolves.toBe(true)
    })

    it("supportsLoadSession routes to the codex sub when an unknown sessionId is passed with agentId=codex — fixes F1", async () => {
        const factory = vi.fn((command: string) =>
            makeStub(command, command === "bunx @agentclientprotocol/codex-acp@latest"))
        const router = createAgentRouter({}, factory)

        await expect(router.supportsLoadSession?.("/ws", "codex")).resolves.toBe(true)
        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/codex-acp@latest", "/ws")
    })

    it("loadSession routes an unknown (restored) sessionId to the codex sub when agentId=codex — fixes F1", async () => {
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)

        await router.loadSession("restored-session", "/ws", "codex")
        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/codex-acp@latest", "/ws")
    })

    it("supportsLoadSession reflects false when the resolved sub does not declare the capability", async () => {
        const router = createAgentRouter({}, (command) => makeStub(command, false))

        await expect(router.supportsLoadSession?.("/ws")).resolves.toBe(false)
    })

    it("aggregates listSessions across all sub-connections", async () => {
        const router = createAgentRouter({}, (command) => makeStub(command))
        await router.newSession("/ws", "pi")
        await router.newSession("/ws", "codex") // 同 cwd、不同 agent → 不同 sub

        const sessions = await router.listSessions("/ws")
        expect(sessions).toHaveLength(2)
        expect(sessions.map((s) => s.id).sort()).toEqual(
            ["bunx @agentclientprotocol/codex-acp@latest-session-1", "bunx pi-acp@latest-session-0"].sort()
        )
    })

    it("prepare routes only trusted curated commands and never claims a session", async () => {
        const stub = makeStub("pi")
        stub.prepare = vi.fn(async () => {})
        const factory = vi.fn(() => stub)
        const router = createAgentRouter({}, factory)

        await router.prepare?.("/ws", "pi")

        expect(factory).toHaveBeenCalledWith("bunx pi-acp@latest", "/ws")
        expect(stub.prepare).toHaveBeenCalledWith("/ws")
        expect(await router.listSessions("/ws")).toEqual([])
    })

    it("refuses to prepare a curated preset whose effective mode is custom", async () => {
        localStorage.setItem(
            "yuzora:agent-settings",
            JSON.stringify({
                preset: "codex",
                command: "",
                traceEnabled: false,
                presetCommands: {
                    codex: { mode: "custom", customCommand: "uvx wrapped-codex" }
                }
            })
        )
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)

        await expect(router.prepare?.("/ws", "codex")).rejects.toThrow("cannot be prepared")
        expect(factory).not.toHaveBeenCalled()
    })

    it("routes session config setters to the owning sub and returns its full response", async () => {
        const stubs = new Map<string, AgentConnection>()
        const router = createAgentRouter({}, (command) => {
            const stub = makeStub(command)
            stub.setSessionConfigOption = vi.fn(async () => [{
                id: command,
                name: "Model",
                category: "model",
                type: "select" as const,
                currentValue: "fast",
                options: [{ value: "fast", name: "Fast" }]
            }])
            stubs.set(command, stub)
            return stub
        })
        const pi = await router.newSession("/ws", "pi")
        await router.newSession("/ws", "codex")

        const result = await router.setSessionConfigOption?.(pi.sessionId, "model", "fast")

        expect(result).toEqual([expect.objectContaining({ id: "bunx pi-acp@latest" })])
        expect(stubs.get("bunx pi-acp@latest")?.setSessionConfigOption)
            .toHaveBeenCalledWith(pi.sessionId, "model", "fast")
        expect(stubs.get("bunx @agentclientprotocol/codex-acp@latest")?.setSessionConfigOption)
            .not.toHaveBeenCalled()
    })

    it("disposes only subs without session ownership", async () => {
        const stubs = new Map<string, AgentConnection>()
        const router = createAgentRouter({}, (command) => {
            const stub = makeStub(command)
            stub.disposePrepared = vi.fn(async () => true)
            stubs.set(command, stub)
            return stub
        })
        await router.prepare?.("/prepared", "pi")
        await router.newSession("/owned", "codex")

        await expect(router.disposePrepared?.("/prepared")).resolves.toBe(true)
        await expect(router.disposePrepared?.("/owned")).resolves.toBe(false)
        expect(stubs.get("bunx pi-acp@latest")?.disposePrepared).toHaveBeenCalledWith("/prepared")
        expect(stubs.get("bunx @agentclientprotocol/codex-acp@latest")?.disposePrepared).not.toHaveBeenCalled()
    })

    it("does not dispose a sub while session ownership is being established", async () => {
        let resolveNewSession!: (value: { sessionId: string; startupInfo: null }) => void
        const stub = makeStub("pi")
        stub.newSession = vi.fn(() => new Promise<{ sessionId: string; startupInfo: null }>(
            (resolve) => { resolveNewSession = resolve }
        ))
        stub.disposePrepared = vi.fn(async () => true)
        const router = createAgentRouter({}, () => stub)

        const pending = router.newSession("/ws", "pi")
        await vi.waitFor(() => expect(stub.newSession).toHaveBeenCalledTimes(1))

        await expect(router.disposePrepared?.("/ws")).resolves.toBe(false)
        expect(stub.disposePrepared).not.toHaveBeenCalled()

        resolveNewSession({ sessionId: "owned", startupInfo: null })
        await expect(pending).resolves.toMatchObject({ sessionId: "owned" })
    })

    it("dropSession forwards to the owning sub-connection — fixes F10", async () => {
        const router = createAgentRouter({}, (command) => makeStub(command))
        const s = await router.newSession("/ws", "pi")

        router.dropSession?.(s.sessionId)

        expect(droppedSessions).toEqual([s.sessionId])
    })

    it("dropSession is a silent no-op for a sessionId that was never routed — fixes F10", () => {
        const router = createAgentRouter({}, (command) => makeStub(command))

        expect(() => router.dropSession?.("never-seen")).not.toThrow()
        expect(droppedSessions).toEqual([])
    })

    it("awaits and propagates cancellation from the owning sub-connection", async () => {
        const stub = makeStub("agent")
        stub.cancel = vi.fn(async () => {
            throw new Error("cancel failed")
        })
        const router = createAgentRouter({}, () => stub)
        const session = await router.newSession("/ws", "pi")

        await expect(router.cancel(session.sessionId)).rejects.toThrow("cancel failed")
        expect(stub.cancel).toHaveBeenCalledWith(session.sessionId)
        await expect(router.cancel("never-routed")).rejects.toThrow("Unknown session")
    })
})
