import { beforeEach, describe, expect, it, vi } from "vitest"

import { AgentAuthRequiredError, type AgentConnection, type StopReason } from "./acpConnection"
import { createAgentRouter, fingerprintAgentCommand } from "./agentRouter"
import { AgentRuntimePrerequisiteError } from "./agentRuntime"

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
        expect(lastPromptCommand()).toBe("bunx @agentclientprotocol/codex-acp@1.1.7")
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
                command: "bunx pi-acp@0.0.32",
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

        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/claude-agent-acp@0.62.0", "/ws")
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
        const router = createAgentRouter({}, (command) => makeStub(command, command === "bunx pi-acp@0.0.32"))

        await expect(router.supportsLoadSession?.("/ws")).resolves.toBe(true)
    })

    it("supportsLoadSession routes to the codex sub when an unknown sessionId is passed with agentId=codex — fixes F1", async () => {
        const factory = vi.fn((command: string) =>
            makeStub(command, command === "bunx @agentclientprotocol/codex-acp@1.1.7"))
        const router = createAgentRouter({}, factory)

        await expect(router.supportsLoadSession?.("/ws", "codex")).resolves.toBe(true)
        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/codex-acp@1.1.7", "/ws")
    })

    it("loadSession routes an unknown (restored) sessionId to the codex sub when agentId=codex — fixes F1", async () => {
        const factory = vi.fn((command: string) => makeStub(command))
        const router = createAgentRouter({}, factory)

        await router.loadSession("restored-session", "/ws", "codex")
        expect(factory).toHaveBeenCalledWith("bunx @agentclientprotocol/codex-acp@1.1.7", "/ws")
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
            ["bunx @agentclientprotocol/codex-acp@1.1.7-session-1", "bunx pi-acp@0.0.32-session-0"].sort()
        )
    })

    it("prepare routes only trusted curated commands and never claims a session", async () => {
        const stub = makeStub("pi")
        stub.prepare = vi.fn(async () => {})
        const factory = vi.fn(() => stub)
        const router = createAgentRouter({}, factory)

        await router.prepare?.("/ws", "pi")

        expect(factory).toHaveBeenCalledWith("bunx pi-acp@0.0.32", "/ws")
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

        expect(result).toEqual([expect.objectContaining({ id: "bunx pi-acp@0.0.32" })])
        expect(stubs.get("bunx pi-acp@0.0.32")?.setSessionConfigOption)
            .toHaveBeenCalledWith(pi.sessionId, "model", "fast")
        expect(stubs.get("bunx @agentclientprotocol/codex-acp@1.1.7")?.setSessionConfigOption)
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
        expect(stubs.get("bunx pi-acp@0.0.32")?.disposePrepared).toHaveBeenCalledWith("/prepared")
        expect(stubs.get("bunx @agentclientprotocol/codex-acp@1.1.7")?.disposePrepared).not.toHaveBeenCalled()
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

    it("keeps a live session routable when another session/new fails on the same sub", async () => {
        const stub = makeStub("pi")
        stub.newSession = vi.fn()
            .mockResolvedValueOnce({ sessionId: "live-session", startupInfo: null })
            .mockRejectedValueOnce(new Error("second session/new failed"))
        stub.prompt = vi.fn(async () => "end_turn" as const)
        stub.cancel = vi.fn()
        stub.disposeOwnerless = vi.fn(async () => true)
        const router = createAgentRouter({ now: () => 0 }, () => stub)

        await router.newSession("/ws", "pi")
        await expect(router.newSession("/ws", "pi")).rejects.toThrow(
            "second session/new failed"
        )
        await Promise.resolve()

        await expect(
            router.prompt("live-session", [{ type: "text", text: "still here?" }])
        ).resolves.toBe("end_turn")
        expect(stub.prompt).toHaveBeenCalledWith(
            "live-session",
            [{ type: "text", text: "still here?" }]
        )
        await router.cancel("live-session")
        expect(stub.cancel).toHaveBeenCalledWith("live-session")

        router.dropSession?.("live-session")
        expect(droppedSessions).toEqual(["live-session"])
        expect(stub.disposeOwnerless).not.toHaveBeenCalled()
    })

    it("keeps a sub while loadSession is still establishing an owner", async () => {
        let resolveLoad!: () => void
        const stub = makeStub("pi")
        stub.loadSession = vi.fn(() => new Promise<void>((resolve) => {
            resolveLoad = resolve
        }))
        stub.newSession = vi.fn(async () => {
            throw new Error("parallel session/new failed")
        })
        stub.prompt = vi.fn(async () => "end_turn" as const)
        stub.disposeOwnerless = vi.fn(async () => true)
        const router = createAgentRouter({ now: () => 0 }, () => stub)

        const loading = router.loadSession("loading-session", "/ws", "pi")
        await vi.waitFor(() => expect(stub.loadSession).toHaveBeenCalledTimes(1))
        await expect(router.newSession("/ws", "pi")).rejects.toThrow(
            "parallel session/new failed"
        )

        resolveLoad()
        await loading
        await expect(
            router.prompt("loading-session", [{ type: "text", text: "loaded?" }])
        ).resolves.toBe("end_turn")
        expect(stub.disposeOwnerless).not.toHaveBeenCalled()
    })

    it("shares one active session/new attempt across a five-click retry storm", async () => {
        let resolveNewSession!: (value: { sessionId: string; startupInfo: null }) => void
        const stub = makeStub("pi")
        stub.newSession = vi.fn(() => new Promise<{ sessionId: string; startupInfo: null }>(
            (resolve) => {
                resolveNewSession = resolve
            }
        ))
        const router = createAgentRouter({ now: () => 0 }, () => stub)

        const attempts = Array.from({ length: 5 }, () => router.newSession("/ws", "pi"))
        await vi.waitFor(() => expect(stub.newSession).toHaveBeenCalledTimes(1))

        resolveNewSession({ sessionId: "storm-owner", startupInfo: null })
        await expect(Promise.all(attempts)).resolves.toEqual(
            Array.from({ length: 5 }, () => expect.objectContaining({ sessionId: "storm-owner" }))
        )
        expect(stub.newSession).toHaveBeenCalledTimes(1)
    })

    it("applies deterministic exponential retry backoff and resets it after success", async () => {
        let now = 0
        let factoryCalls = 0
        const router = createAgentRouter({ now: () => now }, (command) => {
            factoryCalls += 1
            const stub = makeStub(command)
            stub.newSession = factoryCalls <= 2
                ? vi.fn(async () => {
                    throw new Error(`session/new failure ${factoryCalls}`)
                })
                : vi.fn(async () => ({ sessionId: "recovered", startupInfo: null }))
            stub.disposeOwnerless = vi.fn(async () => true)
            return stub
        })

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("session/new failure 1")
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(1_000)

        await expect(router.newSession("/ws", "pi")).rejects.toMatchObject({
            nextAllowedAt: 1_000
        })
        expect(factoryCalls).toBe(1)

        now = 1_000
        await expect(router.newSession("/ws", "pi")).rejects.toThrow("session/new failure 2")
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(3_000)

        now = 3_000
        await expect(router.newSession("/ws", "pi")).resolves.toMatchObject({
            sessionId: "recovered"
        })
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(0)
    })

    it("waits for the previous ownerless teardown before spawning the retry sub", async () => {
        let now = 0
        let factoryCalls = 0
        let resolveTeardown!: () => void
        const teardown = new Promise<void>((resolve) => {
            resolveTeardown = resolve
        })
        const router = createAgentRouter({ now: () => now }, (command) => {
            factoryCalls += 1
            const stub = makeStub(command)
            if (factoryCalls === 1) {
                stub.newSession = vi.fn(async () => {
                    throw new Error("first session/new failed")
                })
                stub.disposeOwnerless = vi.fn(() => teardown.then(() => true))
            }
            return stub
        })

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("first session/new failed")
        now = 1_000
        const retry = router.newSession("/ws", "pi")
        await Promise.resolve()
        await Promise.resolve()
        expect(factoryCalls).toBe(1)

        resolveTeardown()
        await expect(retry).resolves.toMatchObject({
            sessionId: expect.stringContaining("-session-")
        })
        expect(factoryCalls).toBe(2)
    })

    it("keeps the auth-required sub for retry without teardown or backoff", async () => {
        const stub = makeStub("pi")
        stub.newSession = vi.fn()
            .mockRejectedValueOnce(new AgentAuthRequiredError({
                authMethods: [],
                cwd: "/ws",
                sessionId: null
            }))
            .mockResolvedValueOnce({ sessionId: "after-login", startupInfo: null })
        stub.disposeOwnerless = vi.fn(async () => true)
        const factory = vi.fn(() => stub)
        const router = createAgentRouter({ now: () => 0 }, factory)

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("Authentication required")
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(0)
        await expect(router.newSession("/ws", "pi")).resolves.toMatchObject({
            sessionId: "after-login"
        })

        expect(factory).toHaveBeenCalledTimes(1)
        expect(stub.disposeOwnerless).not.toHaveBeenCalled()
    })

    // #37 S3：使用者主動取消冷啟動下載不是連線失敗——比照 auth-required 不進
    // backoff，否則連按取消會一路罰站到 30 秒。
    it("keeps the retry backoff untouched when the user cancels a cold start", async () => {
        const stub = makeStub("pi")
        stub.newSession = vi.fn()
            .mockRejectedValueOnce(new Error("ACP cold start cancelled"))
            .mockResolvedValueOnce({ sessionId: "after-cancel", startupInfo: null })
        stub.disposeOwnerless = vi.fn(async () => true)
        const router = createAgentRouter({ now: () => 0 }, () => stub)

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("ACP cold start cancelled")
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(0)
        // 同一個時間點立刻重試不得被 AgentRetryCooldownError 擋下
        await expect(router.newSession("/ws", "pi")).resolves.toMatchObject({
            sessionId: "after-cancel"
        })
    })

    // #15：缺 runtime 是使用者幾秒內就能修好的外部條件，不是 agent 掛掉——同樣
    // 豁免 backoff，否則被 disable 的正是 prerequisite banner 自己的 Retry 鈕。
    it("keeps the retry backoff untouched when a runtime prerequisite blocks the spawn", async () => {
        const stub = makeStub("pi")
        stub.newSession = vi.fn()
            .mockRejectedValueOnce(new AgentRuntimePrerequisiteError("no runtime", {
                reason: "no-runtime",
                runtimes: { bunx: false, deno: false, node: false, npx: false },
                command: "bunx pi-acp@0.0.32"
            }))
            .mockResolvedValueOnce({ sessionId: "after-install", startupInfo: null })
        stub.disposeOwnerless = vi.fn(async () => true)
        const router = createAgentRouter({ now: () => 0 }, () => stub)

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("no runtime")
        expect(router.retryCooldownUntil?.("/ws", "pi")).toBe(0)
        await expect(router.newSession("/ws", "pi")).resolves.toMatchObject({
            sessionId: "after-install"
        })
    })

    it("reuses the same sub without a fake teardown gate when ownerless disposal returns false", async () => {
        let now = 0
        let factoryCalls = 0
        let resolveDisposal!: (disposed: boolean) => void
        const router = createAgentRouter({ now: () => now }, (command) => {
            factoryCalls += 1
            const stub = makeStub(command)
            if (factoryCalls === 1) {
                stub.newSession = vi.fn()
                    .mockRejectedValueOnce(new Error("process exited during session/new"))
                    .mockResolvedValueOnce({ sessionId: "reused-sub", startupInfo: null })
                stub.disposeOwnerless = vi.fn(() => new Promise<boolean>((resolve) => {
                    resolveDisposal = resolve
                }))
            }
            return stub
        })

        await expect(router.newSession("/ws", "pi")).rejects.toThrow("process exited")
        now = 1_000
        const retry = router.newSession("/ws", "pi")
        await Promise.resolve()
        await Promise.resolve()
        expect(factoryCalls).toBe(1)

        resolveDisposal(false)
        await expect(retry).resolves.toMatchObject({
            sessionId: "reused-sub"
        })
        expect(factoryCalls).toBe(1)
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
