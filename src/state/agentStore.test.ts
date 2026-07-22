import { beforeEach, describe, expect, it, vi } from "vitest"

import {
    AgentAuthRequiredError,
    type AgentConnection,
    type AgentAuthMethod,
    type ElicitationRequest,
    type SessionConfigOption,
    type StopReason
} from "@/agent/acpConnection"
import { createAgentRouter, fingerprintAgentCommand } from "@/agent/agentRouter"
import type { BlockEntry, TranscriptEntry } from "@/agent/acpTypes"
import { AGENT_VERSION_STORAGE_KEY, loadAgentVersions } from "@/agent/agentVersions"
import {
    AGENT_SETTINGS_STORAGE_KEY,
    LAST_USED_CURATED_AGENT_STORAGE_KEY
} from "@/app/workbench/settingsStorage"
import {
    SESSION_INDEX_STORAGE_KEY,
    loadSessionIndex,
    upsertSessionIndexEntry,
    type SessionIndexEntry
} from "./sessionIndexStorage"
import { createAgentStore, resolveSessionTitle, selectWorkspaceAgentCounts, type SessionState } from "./agentStore"

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (reason?: unknown) => void
    const promise = new Promise<T>((res, rej) => {
        resolve = res
        reject = rej
    })
    return { promise, resolve, reject }
}

function fakeConnection(stopReason: StopReason = "end_turn") {
    const promptGate = deferred<StopReason>()
    let promptResult: Promise<StopReason> = Promise.resolve(stopReason)
    const connection: AgentConnection = {
        newSession: vi.fn(async () => ({ sessionId: "s-1", startupInfo: null })),
        loadSession: vi.fn(async () => {}),
        listSessions: vi.fn(async () => []),
        prompt: vi.fn(() => promptResult),
        cancel: vi.fn()
    }
    return {
        connection,
        promptGate,
        holdPrompt() {
            promptResult = promptGate.promise
        },
        rejectPrompt(error: Error) {
            promptResult = Promise.reject(error)
        }
    }
}

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see agentStore.login.test.ts). Install a minimal in-memory
// Storage so settings persistence is exercised for real.
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
    vi.restoreAllMocks()
    installLocalStorage()
    localStorage.clear()
})

const permissionBlock: BlockEntry = {
    id: "perm-1",
    kind: "perm",
    text: "Run command",
    actions: [{ label: "Allow", kind: "allow_once", payload: { optionId: "allow" } }]
}

const terminalAuthMethod: AgentAuthMethod = {
    id: "pi_terminal_login",
    name: "Launch pi in the terminal",
    type: "terminal",
    args: ["--terminal-login"],
    env: {}
}

describe("createAgentStore tone transitions", () => {
    it("records the adapter version only after a trusted session starts successfully", async () => {
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession).mockResolvedValue({
            sessionId: "versioned",
            startupInfo: null,
            agentVersion: "v0.0.32",
            agentIdentity: {
                selectedPreset: "pi",
                commandMode: "latest",
                trustedAgentId: "pi"
            }
        })
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().newSession("/w", "pi")

        expect(store.getState().sessions.get("versioned")?.agentVersion).toBe("v0.0.32")
        expect(loadAgentVersions()).toEqual({ pi: "0.0.32" })
        expect(localStorage.getItem(AGENT_VERSION_STORAGE_KEY)).not.toContain("v0.0.32")
    })

    it("records session/new auth-required state and clears it after retry creates a session", async () => {
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession)
            .mockRejectedValueOnce(new AgentAuthRequiredError({
                authMethods: [terminalAuthMethod],
                cwd: "/w",
                sessionId: null
            }))
            .mockResolvedValueOnce({ sessionId: "s-after-login", startupInfo: null })
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().newSession("/w")).rejects.toThrow("Authentication required")

        expect(store.getState().authRequired).toMatchObject({
            cwd: "/w",
            sessionId: null,
            authMethods: [terminalAuthMethod]
        })
        expect(store.getState().connectionState).toBe("error")

        await expect(store.getState().retryAfterLogin()).resolves.toBe("s-after-login")

        expect(fake.connection.newSession).toHaveBeenNthCalledWith(2, "/w")
        expect(store.getState().authRequired).toBeNull()
        expect(store.getState().activeSessionId).toBe("s-after-login")
        expect(store.getState().connectionState).toBe("ready")
    })

    it("retries login with the originally attempted agent, not the default", async () => {
        const attempts: Array<string | undefined> = []
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession).mockImplementation(async (_cwd, agentId) => {
            attempts.push(agentId)
            if (attempts.length === 1) {
                throw new AgentAuthRequiredError({
                    authMethods: [terminalAuthMethod],
                    cwd: "/ws",
                    sessionId: null
                })
            }
            return { sessionId: "s1", startupInfo: null }
        })
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().newSession("/ws", "codex").catch(() => undefined)
        expect(store.getState().authRequired?.agentId).toBe("codex")

        await store.getState().retryAfterLogin()
        expect(attempts).toEqual(["codex", "codex"])
    })

    it("records prompt auth-required state without replacing it with a generic failure only", async () => {
        const fake = fakeConnection()
        fake.rejectPrompt(new AgentAuthRequiredError({
            authMethods: [terminalAuthMethod],
            cwd: "/w",
            sessionId: "s-1"
        }))
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("/w", "Continue")).rejects.toThrow("Authentication required")

        expect(store.getState().authRequired).toMatchObject({
            cwd: "/w",
            sessionId: "s-1",
            authMethods: [terminalAuthMethod]
        })
        expect(store.getState().sessions.get("s-1")?.error).toBe("Authentication required")
    })

    it("keeps the existing session's agent on a mid-conversation auth failure", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        const sid = await store.getState().newSession("/ws", "codex")

        fake.rejectPrompt(new AgentAuthRequiredError({
            authMethods: [terminalAuthMethod],
            cwd: "/ws",
            sessionId: sid
        }))

        await expect(store.getState().sendPrompt("/ws", "Continue")).rejects.toThrow("Authentication required")

        expect(store.getState().authRequired?.agentId).toBe("codex")
    })

    it("refuses newSession with a relative cwd and never spawns", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().newSession(".")).rejects.toThrow()

        expect(fake.connection.newSession).not.toHaveBeenCalled()
        expect(store.getState().connectionState).toBe("ready")
        expect(store.getState().connectionError).toBeNull()
    })

    it("refuses sendPrompt with a relative cwd and never spawns or prompts", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("./rel", "Hello")).rejects.toThrow()

        expect(fake.connection.newSession).not.toHaveBeenCalled()
        expect(fake.connection.prompt).not.toHaveBeenCalled()
    })

    it("runs while a turn is active, waits on permission, and marks end_turn done", async () => {
        const fake = fakeConnection()
        fake.holdPrompt()
        const store = createAgentStore({ connection: fake.connection })

        const turn = store.getState().sendPrompt("/w", "Fix the failing build")
        await Promise.resolve()
        expect(store.getState().activeSessionId).toBe("s-1")
        expect(store.getState().sessions.get("s-1")?.tone).toBe("run")

        store.getState().onPermissionRequest("s-1", permissionBlock, vi.fn())
        expect(store.getState().sessions.get("s-1")?.tone).toBe("wait")

        fake.promptGate.resolve("end_turn")
        await turn

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("done")
        expect(session?.stopBadge).toBeNull()
        expect(session?.title).toBe("Fix the failing build")
        expect(session?.transcript.every((entry) => !("streaming" in entry) || !entry.streaming)).toBe(true)
    })

    it("image blocks 進 transcript 縮圖；Session Index 序列化不含任何圖片資料（C2 防退化）", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().sendPrompt("/w", [
            { type: "text", text: "look at this" },
            { type: "image", data: "aGVsbG8=", mimeType: "image/png" }
        ])

        const session = store.getState().sessions.get("s-1")
        const userEntry = session?.transcript.find((entry) => "who" in entry && entry.who === "you")
        expect(userEntry).toMatchObject({
            text: "look at this [image]",
            images: [{ mimeType: "image/png", dataUrl: "data:image/png;base64,aGVsbG8=" }]
        })

        // ADR 0001／C2：索引只存識別欄位——整包 localStorage 不得出現圖片位元組。
        const raw = localStorage.getItem("yuzora:agent-sessions") ?? ""
        expect(raw).not.toContain("data:image")
        expect(raw).not.toContain("aGVsbG8=")
        expect(raw).not.toContain("dataUrl")
    })

    it("純圖片 prompt（無文字）也建立帶縮圖的 user entry", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().sendPrompt("/w", [
            { type: "image", data: "eA==", mimeType: "image/webp" }
        ])

        const session = store.getState().sessions.get("s-1")
        const userEntry = session?.transcript.find((entry) => "who" in entry && entry.who === "you")
        expect(userEntry).toMatchObject({
            text: "[image]",
            images: [{ mimeType: "image/webp" }]
        })
    })

    it("marks connection/process prompt errors as fail without losing the transcript", async () => {
        const fake = fakeConnection()
        fake.rejectPrompt(new Error("ACP agent exited"))
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("/w", "Continue")).rejects.toThrow("ACP agent exited")

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("fail")
        expect(session?.error).toBe("ACP agent exited")
        expect(session?.transcript).toEqual([{ id: expect.any(String), who: "you", text: "Continue", streaming: false }])
        expect(store.getState().connectionState).toBe("error")
        expect(store.getState().connectionError).toBe("ACP agent exited")
    })

    it("writes a human-readable connectionError when sendPrompt fails before a session exists", async () => {
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession).mockRejectedValueOnce(
            new Error("ACP initialize timed out after 60000ms")
        )
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("/w", "Hello")).rejects.toThrow(
            "ACP initialize timed out after 60000ms"
        )

        expect(store.getState().connectionState).toBe("error")
        expect(store.getState().connectionError).toBe("ACP initialize timed out after 60000ms")
        expect(store.getState().activeSessionId).toBeNull()
    })

    it("clears connectionError as soon as the next attempt starts, before it resolves", async () => {
        const fake = fakeConnection()
        fake.rejectPrompt(new Error("ACP agent exited"))
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("/w", "Continue")).rejects.toThrow("ACP agent exited")
        expect(store.getState().connectionError).toBe("ACP agent exited")

        fake.holdPrompt()
        const retry = store.getState().sendPrompt("/w", "Continue again")
        expect(store.getState().connectionError).toBeNull()

        fake.promptGate.resolve("end_turn")
        await retry
        expect(store.getState().connectionError).toBeNull()
    })

    it("keeps cancelled turns idle-looking with no error badge", async () => {
        const fake = fakeConnection("cancelled")
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().sendPrompt("/w", "Stop this")

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("idle")
        expect(session?.stopReason).toBe("cancelled")
        expect(session?.stopBadge).toBeNull()
        expect(session?.error).toBeNull()
        expect(session?.transcript.some((entry) => "kind" in entry && entry.kind === "error")).toBe(false)
    })

    it("records pending permission without duplicating the runtime-appended block", () => {
        const choose = vi.fn()
        const store = createAgentStore({ connection: fakeConnection().connection })

        // replaceTranscript no longer creates ghost sessions (F3) — seed it first.
        store.getState().selectSession("s-1")
        store.getState().replaceTranscript("s-1", [permissionBlock])
        store.getState().onPermissionRequest("s-1", permissionBlock, choose)

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("wait")
        expect(session?.transcript.filter((entry) => "kind" in entry && entry.kind === "perm")).toHaveLength(1)
        expect(store.getState().pendingPermissions.get("s-1")).toMatchObject({
            request: {
                text: "Run command",
                actions: permissionBlock.actions
            }
        })
        expect(store.getState().pendingPermissions.get("s-1")?.choose).toBe(choose)
    })

    it("responds to a pending permission and resumes the running turn", () => {
        const choose = vi.fn()
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().replaceTranscript("s-1", [permissionBlock])
        store.getState().onPermissionRequest("s-1", permissionBlock, choose)
        store.getState().respondPermission("s-1", "allow")

        expect(choose).toHaveBeenCalledExactlyOnceWith("allow")
        expect(store.getState().pendingPermissions.has("s-1")).toBe(false)
        expect(store.getState().sessions.get("s-1")?.tone).toBe("run")
    })

    it("clears pending permission state only after cancelling a session succeeds", async () => {
        const choose = vi.fn()
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        store.getState().replaceTranscript("s-1", [permissionBlock])
        store.getState().onPermissionRequest("s-1", permissionBlock, choose)
        await store.getState().cancel("s-1")

        expect(fake.connection.cancel).toHaveBeenCalledExactlyOnceWith("s-1")
        expect(choose).not.toHaveBeenCalled()
        expect(store.getState().pendingPermissions.has("s-1")).toBe(false)
        expect(store.getState().sessions.get("s-1")).toMatchObject({
            tone: "idle",
            stopReason: "cancelled",
            error: null
        })
    })

    it("preserves the pending turn and permission when backend cancel rejects", async () => {
        const fake = fakeConnection()
        vi.mocked(fake.connection.cancel).mockRejectedValueOnce(new Error("cancel transport failed"))
        const store = createAgentStore({ connection: fake.connection })
        const choose = vi.fn()
        store.getState().selectSession("s-1")
        const session = store.getState().sessions.get("s-1")!
        store.setState({
            sessions: new Map([["s-1", {
                ...session,
                tone: "run",
                running: true,
                pendingTurn: true
            }]]),
            pendingPermissions: new Map([["s-1", {
                entryId: permissionBlock.id,
                request: {
                    text: permissionBlock.text,
                    actions: permissionBlock.actions ?? []
                },
                choose
            }]])
        })

        await expect(store.getState().cancel("s-1")).rejects.toThrow("cancel transport failed")

        expect(store.getState().sessions.get("s-1")).toMatchObject({
            tone: "run",
            running: true,
            pendingTurn: true,
            stopReason: null
        })
        expect(store.getState().pendingPermissions.has("s-1")).toBe(true)
        expect(store.getState().connectionError).toBe("cancel transport failed")
    })

    it("records the chosen option per perm entry when a permission is answered", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const choose = vi.fn()
        store.getState().selectSession("s-1")
        store.getState().onPermissionRequest("s-1", permissionBlock, choose)
        expect(store.getState().sessions.get("s-1")?.tone).toBe("wait")

        store.getState().respondPermission("s-1", "allow")

        expect(choose).toHaveBeenCalledExactlyOnceWith("allow")
        expect(store.getState().pendingPermissions.has("s-1")).toBe(false)
        expect(store.getState().sessions.get("s-1")).toMatchObject({
            tone: "run",
            permissionOutcomes: { [permissionBlock.id]: "allow" }
        })
    })

    it("does not overwrite a turn that completes naturally while cancel is in flight", async () => {
        const gate = deferred<void>()
        const fake = fakeConnection()
        vi.mocked(fake.connection.cancel).mockReturnValueOnce(gate.promise)
        const store = createAgentStore({ connection: fake.connection })
        store.getState().selectSession("s-1")
        const session = store.getState().sessions.get("s-1")!
        store.setState({
            sessions: new Map([["s-1", {
                ...session,
                tone: "run",
                running: true,
                pendingTurn: true
            }]])
        })

        const cancelling = store.getState().cancel("s-1")
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("s-1", {
                ...sessions.get("s-1")!,
                tone: "done",
                running: false,
                pendingTurn: false,
                stopReason: "end_turn"
            })
            return { sessions }
        })
        gate.resolve()

        await expect(cancelling).resolves.toBe(true)
        expect(store.getState().sessions.get("s-1")).toMatchObject({
            tone: "done",
            pendingTurn: false,
            stopReason: "end_turn"
        })
    })

    it.each([
        ["refusal", "refusal"],
        ["max_tokens", "truncated"],
        ["max_turn_requests", "truncated"]
    ] as const)("surfaces %s as a non-plain done state", async (stopReason, badgeKind) => {
        const fake = fakeConnection(stopReason)
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().sendPrompt("/w", "Explain the risky change")

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("done")
        expect(session?.stopReason).toBe(stopReason)
        expect(session?.stopBadge?.kind).toBe(badgeKind)
        expect(session?.transcript.some((entry) => {
            if (!("kind" in entry)) return false
            const meta = entry.meta ? JSON.parse(entry.meta) as { stopReason?: string } : {}
            return meta.stopReason === stopReason
        })).toBe(true)
        if (stopReason === "refusal") {
            expect(session?.transcript.some((entry) => "kind" in entry && entry.kind === "error")).toBe(true)
        }
    })
})

describe("createAgentStore transcript, slash, and session switching", () => {
    it("feeds updates through reduceSessionUpdate and keeps metadata title priority", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().upsertSessionMeta({ id: "s-meta", cwd: "/w", name: "Named pi session" })
        store.getState().appendUpdate("s-meta", {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "First prompt should not replace metadata title" }
        })
        store.getState().appendUpdate("s-meta", {
            sessionUpdate: "current_mode_update",
            currentModeId: "plan"
        })

        const session = store.getState().sessions.get("s-meta")
        expect(session?.title).toBe("Named pi session")
        expect(session?.mode).toBe("plan")
        expect(session?.transcript).toEqual([
            { id: expect.any(String), who: "you", text: "First prompt should not replace metadata title", streaming: true }
        ])
    })

    it("does not replace a prompt-derived title when metadata has no name", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().appendUpdate("s-plain", {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "Keep this title" }
        })
        store.getState().upsertSessionMeta({ id: "s-plain", cwd: "/w" })

        expect(store.getState().sessions.get("s-plain")?.title).toBe("Keep this title")
    })

    it("stores slash commands per session and filters by slash prefix", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().setAvailableCommands("s-1", [
            { name: "fix", description: "Fix selected issue" },
            { name: "format", description: "Format the current file" },
            { name: "explain", description: "Explain code" }
        ])
        store.getState().setAvailableCommands("s-2", [
            { name: "test", description: "Run tests" }
        ])

        expect(store.getState().filterCommands("/f", "s-1").map((command) => command.name)).toEqual([
            "fix",
            "format"
        ])
        expect(store.getState().filterCommands("ex", "s-1").map((command) => command.name)).toEqual([
            "explain"
        ])
        expect(store.getState().filterCommands("/", "s-2").map((command) => command.name)).toEqual([
            "test"
        ])
    })

    it("switches active sessions and creates a new session through the injected connection", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        expect(store.getState().connectionState).toBe("ready")
        store.getState().upsertSessionMeta({ id: "existing", cwd: "/w" })
        store.getState().selectSession("existing")
        expect(store.getState().activeSessionId).toBe("existing")

        await store.getState().newSession("/w")

        expect(fake.connection.newSession).toHaveBeenCalledWith("/w")
        expect(store.getState().activeSessionId).toBe("s-1")
        expect(store.getState().sessions.has("s-1")).toBe(true)
    })

    it("stamps the chosen agentId onto the created session", async () => {
        const created: Array<[string, string | undefined]> = []
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession).mockImplementation(async (cwd, agentId) => {
            created.push([cwd, agentId])
            return { sessionId: "s1", startupInfo: null }
        })
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().newSession("/ws", "codex")

        expect(created).toEqual([["/ws", "codex"]])
        expect(store.getState().sessions.get("s1")!.agentId).toBe("codex")
    })

    it("defaults a session's agentId from the global setting when none is picked", async () => {
        localStorage.setItem(
            AGENT_SETTINGS_STORAGE_KEY,
            JSON.stringify({ preset: "claude", command: "", traceEnabled: false })
        )
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession).mockResolvedValue({ sessionId: "s2", startupInfo: null })
        const store = createAgentStore({ connection: fake.connection })

        await store.getState().newSession("/ws")

        expect(store.getState().sessions.get("s2")!.agentId).toBe("claude")
    })

    it("keeps an existing session's agentId when sendPrompt implicitly resumes it", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        const sid = await store.getState().newSession("/ws", "codex")
        expect(store.getState().sessions.get(sid)!.agentId).toBe("codex")

        await store.getState().sendPrompt("/ws", "hi")

        expect(store.getState().sessions.get(sid)!.agentId).toBe("codex")
    })

    it("does not let running=false session info overwrite a failed session", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const transcript: TranscriptEntry[] = [{ id: "t0", who: "agent", text: "partial", streaming: true }]

        store.getState().replaceTranscript("s-1", transcript)
        store.getState().markConnectionError("s-1", new Error("process died"))
        store.getState().onSessionInfo("s-1", { queueDepth: 0, running: false })

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("fail")
        expect(session?.queueDepth).toBe(0)
        expect(session?.running).toBe(false)
    })

    it("marks the session restored on a connection error, so continueSession takes the load path — fixes F2", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        await store.getState().newSession("/ws", "pi")
        const sessionId = store.getState().activeSessionId!

        store.getState().markConnectionError(sessionId, new Error("agent process exited"))
        expect(store.getState().sessions.get(sessionId)?.restored).toBe(true)

        await store.getState().continueSession(sessionId)

        expect(fake.connection.supportsLoadSession).toHaveBeenCalledWith("/ws", "pi")
        expect(fake.connection.loadSession).toHaveBeenCalled()
        expect(store.getState().sessions.get(sessionId)?.restored).toBe(false)
    })

    it("shows run tone during a turn without any session_info_update (claude/codex path)", async () => {
        const fake = fakeConnection()
        fake.holdPrompt()
        const store = createAgentStore({ connection: fake.connection })

        const turn = store.getState().sendPrompt("/ws", "hi")
        await vi.waitFor(() => expect(store.getState().sessions.get("s-1")!.tone).toBe("run"))

        fake.promptGate.resolve("end_turn")
        await turn

        expect(store.getState().sessions.get("s-1")!.tone).toBe("done")
    })

    it("P10-B: sendPrompt on another workspace creates a fresh session, not hijacking the active one", async () => {
        const connection = {
            newSession: vi.fn(async (cwd: string) => ({ sessionId: cwd === "/ws-a" ? "sa" : "sb", startupInfo: null })),
            loadSession: async () => {},
            listSessions: async () => [],
            prompt: async () => "end_turn" as StopReason,
            cancel: () => {}
        }
        const store = createAgentStore({ connection: connection as never })

        await store.getState().newSession("/ws-a") // activeSessionId = "sa", cwd /ws-a
        expect(store.getState().activeSessionId).toBe("sa")

        await store.getState().sendPrompt("/ws-b", "hi") // 在 workspace B 送 prompt

        // 不得把 sa 拿來用；應建立 /ws-b 的新 session
        expect(store.getState().sessions.get("sa")!.cwd).toBe("/ws-a") // sa.cwd 未被 /ws-b 污染
        expect(store.getState().sessions.has("sb")).toBe(true)
    })

    it("P7: appends exactly one interrupt marker when cancel and cancelled both fire", async () => {
        let resolvePrompt!: (r: StopReason) => void
        const connection = {
            newSession: async () => ({ sessionId: "s1", startupInfo: null }),
            loadSession: async () => {},
            listSessions: async () => [],
            prompt: () =>
                new Promise<StopReason>((r) => {
                    resolvePrompt = r
                }),
            cancel: () => {}
        }
        const store = createAgentStore({ connection: connection as never })

        const turn = store.getState().sendPrompt("/ws", "hi")
        await vi.waitFor(() => expect(store.getState().sessions.has("s1")).toBe(true))
        await store.getState().cancel("s1")
        resolvePrompt("cancelled")
        await turn

        const t = store.getState().sessions.get("s1")!.transcript
        const markers = t.filter((e) => "kind" in e && typeof e.meta === "string" && e.meta.includes("interrupted"))
        expect(markers).toHaveLength(1)
    })

    it("P7: replaceTranscript keeps the interrupt marker when a late onTranscript arrives without it", async () => {
        let resolvePrompt!: (r: StopReason) => void
        const connection = {
            newSession: async () => ({ sessionId: "s1", startupInfo: null }),
            loadSession: async () => {},
            listSessions: async () => [],
            prompt: () =>
                new Promise<StopReason>((r) => {
                    resolvePrompt = r
                }),
            cancel: () => {}
        }
        const store = createAgentStore({ connection: connection as never })

        const turn = store.getState().sendPrompt("/ws", "hi")
        await vi.waitFor(() => expect(store.getState().sessions.has("s1")).toBe(true))
        await store.getState().cancel("s1")
        resolvePrompt("cancelled")
        await turn

        const withMarker = store.getState().sessions.get("s1")!.transcript
        const markerBefore = withMarker.filter(
            (e) => "kind" in e && typeof e.meta === "string" && e.meta.includes("interrupted")
        )
        expect(markerBefore).toHaveLength(1) // sanity: 取消後標記已存在

        // 模擬 late onTranscript：連線層 transcript 沒有中斷標記，直接覆蓋。
        store.getState().replaceTranscript("s1", [{ id: "t0", who: "agent", text: "late chunk", streaming: false }])

        const after = store.getState().sessions.get("s1")!.transcript
        const markerAfter = after.filter(
            (e) => "kind" in e && typeof e.meta === "string" && e.meta.includes("interrupted")
        )
        expect(markerAfter).toHaveLength(1) // 標記沒被 late transcript 蓋掉
        expect(after.at(-1)).toMatchObject(markerAfter[0])
    })

    it("P1: pendingNewSession is true while creating and false after", async () => {
        let resolveNew!: (v: { sessionId: string; startupInfo: string | null }) => void
        const connection = {
            newSession: () =>
                new Promise<{ sessionId: string; startupInfo: string | null }>((r) => {
                    resolveNew = r
                }),
            loadSession: async () => {},
            listSessions: async () => [],
            prompt: async () => "end_turn" as StopReason,
            cancel: () => {}
        }
        const store = createAgentStore({ connection: connection as never })

        const p = store.getState().newSession("/ws")
        expect(store.getState().pendingNewSession).toBe(true)
        resolveNew({ sessionId: "s1", startupInfo: null })
        await p
        expect(store.getState().pendingNewSession).toBe(false)
    })
})

describe("usage and agent-title dispatch", () => {
    it("setUsage writes the usage snapshot onto the session", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        // seed the session; setUsage no longer creates ghost sessions, and (F3) neither does replaceTranscript.
        store.getState().selectSession("s-1")
        store.getState().replaceTranscript("s-1", [])

        store.getState().setUsage("s-1", { used: 120, size: 1000, cost: { amount: 0.01, currency: "USD" } })

        expect(store.getState().sessions.get("s-1")?.usage).toEqual({
            used: 120,
            size: 1000,
            cost: { amount: 0.01, currency: "USD" }
        })
    })

    it("updates used/size, preserves only same-session cost when omitted, and fully replaces incoming cost", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        store.getState().selectSession("s-1")
        store.getState().selectSession("s-2")

        store.getState().setUsage("s-1", {
            used: 10,
            size: 100,
            cost: { amount: 0, currency: "EUR" }
        })
        store.getState().setUsage("s-1", { used: 25, size: 200 })
        store.getState().setUsage("s-2", { used: 1, size: 50 })

        expect(store.getState().sessions.get("s-1")?.usage).toEqual({
            used: 25,
            size: 200,
            cost: { amount: 0, currency: "EUR" }
        })
        expect(store.getState().sessions.get("s-2")?.usage).toEqual({ used: 1, size: 50 })

        store.getState().setUsage("s-1", {
            used: 30,
            size: 300,
            cost: { amount: 0.000001, currency: "JPY" }
        })
        expect(store.getState().sessions.get("s-1")?.usage).toEqual({
            used: 30,
            size: 300,
            cost: { amount: 0.000001, currency: "JPY" }
        })
    })

    it("applyAgentTitle writes agentTitle and the resolved title follows it", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().appendUpdate("s-1", {
            sessionUpdate: "user_message_chunk",
            content: { type: "text", text: "derived from prompt" }
        })
        expect(store.getState().sessions.get("s-1")?.derivedTitle).toBe("derived from prompt")
        expect(store.getState().sessions.get("s-1")?.agentTitle).toBeUndefined()

        store.getState().applyAgentTitle("s-1", "Fix login bug")

        const session = store.getState().sessions.get("s-1")
        expect(session?.agentTitle).toBe("Fix login bug")
        expect(session?.title).toBe("Fix login bug")
    })

    it("keeps showing the sessionAlias while applyAgentTitle still updates agentTitle underneath", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().replaceTranscript("s-1", [])
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            const current = sessions.get("s-1")!
            sessions.set("s-1", { ...current, title: "My custom name", sessionAlias: "My custom name" })
            return { sessions }
        })

        store.getState().applyAgentTitle("s-1", "Agent suggested title")

        const session = store.getState().sessions.get("s-1")
        // title 仍顯示 alias，沒被覆寫
        expect(session?.title).toBe("My custom name")
        // 但 agentTitle 底下確實有更新，一旦 alias 清掉就能看見
        expect(session?.agentTitle).toBe("Agent suggested title")
    })

    it("resolveSessionTitle falls back to agentTitle once sessionAlias is cleared to null", () => {
        const session: Pick<SessionState, "sessionAlias" | "agentTitle" | "derivedTitle"> = {
            sessionAlias: "My custom name",
            agentTitle: "Agent suggested title",
            derivedTitle: "derived from prompt"
        }
        expect(resolveSessionTitle(session)).toBe("My custom name")

        expect(resolveSessionTitle({ ...session, sessionAlias: null })).toBe("Agent suggested title")
    })

    it("resolveSessionTitle falls back to derivedTitle, then DEFAULT_SESSION_TITLE, when nothing else is set", () => {
        expect(resolveSessionTitle({ derivedTitle: "derived from prompt" })).toBe("derived from prompt")
        expect(resolveSessionTitle({})).toBe("New session")
    })
})

describe("authoritative session config", () => {
    const modelOption = (
        currentValue: string,
        values = [currentValue]
    ): SessionConfigOption => ({
        id: "runtime-model",
        name: "Model",
        category: "model",
        type: "select",
        currentValue,
        options: values.map((value) => ({ value, name: value }))
    })

    const effortOption = (currentValue: string): SessionConfigOption => ({
        id: "runtime-effort",
        name: "Effort",
        category: "thought_level",
        type: "select",
        currentValue,
        options: [{ value: currentValue, name: currentValue }]
    })

    it("keeps a notification's full replacement when an older setter response arrives late", async () => {
        const setter = deferred<SessionConfigOption[]>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: "s-1",
            startupInfo: null,
            configOptions: [modelOption("old"), effortOption("low")]
        }))
        connection.setSessionConfigOption = vi.fn(() => setter.promise)
        const store = createAgentStore({ connection })
        await store.getState().newSession("/ws")

        const request = store.getState().setSessionConfigOption("s-1", "runtime-model", "requested")

        expect(store.getState().sessions.get("s-1")?.configOptions).toEqual([
            modelOption("old"),
            effortOption("low")
        ])
        expect(store.getState().sessions.get("s-1")?.configRequest).toMatchObject({
            configId: "runtime-model",
            value: "requested"
        })

        store.getState().replaceConfigOptions("s-1", [effortOption("notification-high")])
        setter.resolve([modelOption("late-old-response")])
        await request

        const session = store.getState().sessions.get("s-1")
        expect(session?.configOptions).toEqual([effortOption("notification-high")])
        expect(session?.configRequest).toBeNull()
        expect(session?.configRevision).toBe(2)
    })

    // soak 回饋 #1/#5：turn 進行中允許改 config（pi 隨時可切）；僅同 session 的
    // setter 單飛鎖保留。
    it("rejects same-session pending setters while allowing another session or an active turn", async () => {
        const first = deferred<SessionConfigOption[]>()
        let nextSession = 0
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: `s-${++nextSession}`,
            startupInfo: null,
            configOptions: [modelOption("old", ["old", "new"])]
        }))
        connection.setSessionConfigOption = vi.fn((sessionId) =>
            sessionId === "s-1" ? first.promise : Promise.resolve([modelOption("new")])
        )
        const store = createAgentStore({ connection })
        await store.getState().newSession("/ws")
        await store.getState().sendPrompt("/ws", "promote first session")
        await store.getState().newSession("/ws")

        const pending = store.getState().setSessionConfigOption("s-1", "runtime-model", "new")
        await expect(
            store.getState().setSessionConfigOption("s-1", "runtime-model", "new")
        ).rejects.toThrow(/pending/i)

        await expect(
            store.getState().setSessionConfigOption("s-2", "runtime-model", "new")
        ).resolves.toEqual([modelOption("new")])

        // turn 進行中照樣可切（下一次 LLM 呼叫生效）。
        store.getState().onSessionInfo("s-2", { queueDepth: 0, running: true })
        await expect(
            store.getState().setSessionConfigOption("s-2", "runtime-model", "old")
        ).resolves.toEqual([modelOption("new")])

        expect(connection.setSessionConfigOption).toHaveBeenCalledTimes(3)
        first.resolve([modelOption("new")])
        await pending
    })

    it("preserves the old snapshot on failure, exposes the error, and replaces it on retry", async () => {
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: "s-1",
            startupInfo: null,
            configOptions: [modelOption("old"), effortOption("low")]
        }))
        connection.setSessionConfigOption = vi.fn()
            .mockRejectedValueOnce(new Error("Adapter rejected the model"))
            .mockResolvedValueOnce([effortOption("high")])
        const store = createAgentStore({ connection })
        await store.getState().newSession("/ws")

        await expect(
            store.getState().setSessionConfigOption("s-1", "runtime-model", "new")
        ).rejects.toThrow("Adapter rejected the model")

        expect(store.getState().sessions.get("s-1")).toMatchObject({
            configOptions: [modelOption("old"), effortOption("low")],
            configRequest: null,
            configError: "Adapter rejected the model",
            configRevision: 1
        })

        await expect(
            store.getState().setSessionConfigOption("s-1", "runtime-model", "new")
        ).resolves.toEqual([effortOption("high")])
        expect(store.getState().sessions.get("s-1")).toMatchObject({
            configOptions: [effortOption("high")],
            configRequest: null,
            configError: null,
            configRevision: 2
        })
    })

    it("treats a load result as a full replacement and removes options absent from it", async () => {
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: "s-1",
            startupInfo: null,
            configOptions: [modelOption("old"), effortOption("low")]
        }))
        connection.supportsLoadSession = vi.fn(async () => true)
        connection.loadSession = vi.fn(async () => ({
            startupInfo: null,
            configOptions: [effortOption("loaded-high")]
        }))
        const store = createAgentStore({ connection })
        await store.getState().newSession("/ws")
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("s-1", { ...sessions.get("s-1")!, restored: true })
            return { sessions }
        })

        await store.getState().continueSession("s-1")

        expect(store.getState().sessions.get("s-1")).toMatchObject({
            restored: false,
            configOptions: [effortOption("loaded-high")],
            configRevision: 2
        })
    })

    it("reset invalidates an old setter token without clearing a new request for the reused session id", async () => {
        const first = deferred<SessionConfigOption[]>()
        const second = deferred<SessionConfigOption[]>()
        let setterCount = 0
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: "s-1",
            startupInfo: null,
            configOptions: [modelOption("old", ["old", "new"])]
        }))
        connection.setSessionConfigOption = vi.fn(() => (++setterCount === 1 ? first.promise : second.promise))
        const store = createAgentStore({ connection })
        await store.getState().newSession("/ws")
        const staleRequest = store.getState().setSessionConfigOption("s-1", "runtime-model", "new")

        store.getState().reset()
        await store.getState().newSession("/ws")
        const currentRequest = store.getState().setSessionConfigOption("s-1", "runtime-model", "new")

        first.resolve([modelOption("late-stale")])
        await staleRequest
        expect(store.getState().sessions.get("s-1")?.configRequest).toMatchObject({
            configId: "runtime-model",
            value: "new"
        })
        expect(store.getState().sessions.get("s-1")?.configOptions).toEqual([
            modelOption("old", ["old", "new"])
        ])

        second.resolve([modelOption("new")])
        await currentRequest
        expect(store.getState().sessions.get("s-1")?.configOptions).toEqual([modelOption("new")])
        expect(store.getState().sessions.get("s-1")?.configRequest).toBeNull()
    })
})

describe("setUsage/applyAgentTitle do not create ghost sessions", () => {
    it("ignores setUsage for an unknown sessionId without creating a session", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        expect(store.getState().sessions.size).toBe(0)

        store.getState().setUsage("unknown-session", { used: 1, size: 10 })

        expect(store.getState().sessions.size).toBe(0)
    })

    it("ignores applyAgentTitle for an unknown sessionId without creating a session", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        expect(store.getState().sessions.size).toBe(0)

        store.getState().applyAgentTitle("unknown-session", "Some title")

        expect(store.getState().sessions.size).toBe(0)
    })
})

describe("Phase 4: emptySession defaults", () => {
    it("a freshly-created session (no explicit agentId) defaults agentId to undefined, not \"pi\"", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().selectSession("brand-new")

        expect(store.getState().sessions.get("brand-new")?.agentId).toBeUndefined()
    })
})

describe("Phase 4: Session Index sync", () => {
    it("keeps an explicit blank session ephemeral until its first prompt, then persists it for restore", async () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        await store.getState().newSession("/ws", "codex")

        expect(store.getState().sessions.get("s-1")?.ephemeral).toBe(true)
        expect(loadSessionIndex()).toEqual([])

        await store.getState().sendPrompt("/ws", "hi")

        expect(store.getState().sessions.get("s-1")?.ephemeral).toBe(false)
        const entries = loadSessionIndex()
        expect(entries).toHaveLength(1)
        expect(entries[0]).toMatchObject({ sessionId: "s-1", cwd: "/ws", agentId: "codex" })

        const restarted = createAgentStore({ connection: fakeConnection().connection })
        restarted.getState().hydrateRestoredSessions(entries)
        expect(restarted.getState().sessions.get("s-1")).toMatchObject({
            agentId: "codex",
            restored: true
        })
    })

    it("persists only a fingerprint for a custom command, never the command or its secret", async () => {
        const command = "uvx private-acp --token super-secret"
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "custom",
            command,
            traceEnabled: false
        }))
        const router = createAgentRouter({}, () => fakeConnection().connection)
        const store = createAgentStore({ connection: router })

        await store.getState().newSession("/ws")

        expect(loadSessionIndex()).toEqual([])
        expect(localStorage.getItem(SESSION_INDEX_STORAGE_KEY) ?? "").not.toContain("super-secret")

        await store.getState().sendPrompt("/ws", "hi")

        const [entry] = loadSessionIndex()
        expect(entry).toMatchObject({
            sessionId: "s-1",
            agentId: "custom",
            customCommandFingerprint: expect.stringMatching(/^sha256:[0-9a-f]{64}$/)
        })
        const rawIndex = localStorage.getItem(SESSION_INDEX_STORAGE_KEY) ?? ""
        expect(rawIndex).not.toContain("private-acp")
        expect(rawIndex).not.toContain("super-secret")
    })

    it("persists the custom command fingerprint for sendPrompt's implicit session creation", async () => {
        const command = "uvx private-acp --token implicit-secret"
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "custom",
            command,
            traceEnabled: false
        }))
        const router = createAgentRouter({}, () => fakeConnection().connection)
        const store = createAgentStore({ connection: router })

        await store.getState().sendPrompt("/ws", "hi")

        const [entry] = loadSessionIndex()
        expect(entry).toMatchObject({
            agentId: "custom",
            customCommandFingerprint: expect.stringMatching(/^sha256:[0-9a-f]{64}$/)
        })
        const rawIndex = localStorage.getItem(SESSION_INDEX_STORAGE_KEY) ?? ""
        expect(rawIndex).not.toContain("private-acp")
        expect(rawIndex).not.toContain("implicit-secret")
    })

    it("downgrades a curated preset's Custom mode to untrusted for implicit sessions", async () => {
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "codex",
            command: "",
            traceEnabled: false,
            presetCommands: {
                codex: { mode: "custom", customCommand: "uvx wrapped-codex --token private" }
            }
        }))
        const router = createAgentRouter({}, () => fakeConnection().connection)
        const store = createAgentStore({ connection: router })

        await store.getState().sendPrompt("/ws", "hi")

        const session = store.getState().sessions.get("s-1")
        expect(session?.agentId).toBe("custom")
        expect(session?.customCommandFingerprint).toMatch(/^sha256:/)
        const [entry] = loadSessionIndex()
        expect(entry).toMatchObject({
            agentId: "custom",
            customCommandFingerprint: expect.stringMatching(/^sha256:/)
        })
        expect(localStorage.getItem(SESSION_INDEX_STORAGE_KEY)).not.toContain("wrapped-codex")
    })

    it("sendPrompt's implicit session creation also upserts a Session Index entry", async () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        await store.getState().sendPrompt("/ws", "hi")

        const entries = loadSessionIndex()
        expect(entries).toHaveLength(1)
        expect(entries[0]).toMatchObject({ sessionId: "s-1", cwd: "/ws" })
    })

    it("applyAgentTitle touches the indexed entry's agentTitle", async () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        await store.getState().newSession("/ws")

        store.getState().applyAgentTitle("s-1", "Fix login bug")

        expect(loadSessionIndex()).toEqual([])
        await store.getState().sendPrompt("/ws", "hi")
        expect(loadSessionIndex()[0]).toMatchObject({ agentTitle: "Fix login bug" })
    })

    it("a completed turn (sendPrompt resolve) touches lastActiveAt", async () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        await store.getState().newSession("/ws")
        await store.getState().sendPrompt("/ws", "first")
        const createdAt = loadSessionIndex()[0].lastActiveAt

        await new Promise((r) => setTimeout(r, 5))
        await store.getState().sendPrompt("/ws", "second")

        expect(loadSessionIndex()[0].lastActiveAt).toBeGreaterThan(createdAt)
    })

    it("touching a session with no cwd (never indexed) is a silent no-op", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        store.getState().replaceTranscript("s-1", [])

        store.getState().applyAgentTitle("s-1", "Untracked title")

        expect(loadSessionIndex()).toEqual([])
    })
})

describe("Phase P2: one visible ephemeral draft", () => {
    it("returns the same in-flight promise for normalized cwd + effective identity", async () => {
        const gate = deferred<{
            sessionId: string
            startupInfo: null
            agentIdentity: { selectedPreset: "pi"; commandMode: "latest"; trustedAgentId: "pi" }
        }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(() => gate.promise)
        connection.dropSession = vi.fn()
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        const first = store.getState().ensureDraftSession("/ws/", "pi")
        const second = store.getState().ensureDraftSession("/ws", "pi")

        expect(first).toBe(second)
        expect(connection.newSession).toHaveBeenCalledTimes(1)
        gate.resolve({
            sessionId: "draft-1",
            startupInfo: null,
            agentIdentity: { selectedPreset: "pi", commandMode: "latest", trustedAgentId: "pi" }
        })
        await expect(first).resolves.toBe("draft-1")
        expect(store.getState().activeSessionId).toBe("draft-1")
        expect(store.getState().sessions.get("draft-1")).toMatchObject({
            cwd: "/ws/",
            agentId: "pi",
            ephemeral: true,
            transcript: []
        })
        expect(loadSessionIndex()).toEqual([])
        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBe("pi")
    })

    it("preserves callbacks that arrive before a fresh session/new result is registered", async () => {
        const gate = deferred<{
            sessionId: string
            startupInfo: null
            agentIdentity: { selectedPreset: "pi"; commandMode: "latest"; trustedAgentId: "pi" }
        }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(() => gate.promise)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        const draft = store.getState().ensureDraftSession("/ws", "pi")
        store.getState().appendUpdate("fresh-id", {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "startup callback" }
        })
        store.getState().setAvailableCommands("fresh-id", [{ name: "help", description: "Help" }])

        gate.resolve({
            sessionId: "fresh-id",
            startupInfo: null,
            agentIdentity: { selectedPreset: "pi", commandMode: "latest", trustedAgentId: "pi" }
        })
        await expect(draft).resolves.toBe("fresh-id")

        expect(store.getState().sessions.get("fresh-id")?.transcript).not.toHaveLength(0)
        expect(store.getState().sessions.get("fresh-id")?.availableCommands).toEqual([
            { name: "help", description: "Help" }
        ])
    })

    it("joins concurrent explicit New to the same pending auto draft, then keeps later New intentional", async () => {
        const firstCreation = deferred<{ sessionId: string; startupInfo: null }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn()
            .mockImplementationOnce(() => firstCreation.promise)
            .mockResolvedValueOnce({ sessionId: "next-explicit", startupInfo: null })
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        const automatic = store.getState().ensureDraftSession("/ws", "pi")
        const explicit = store.getState().newSession("/ws/", "pi")

        expect(connection.newSession).toHaveBeenCalledTimes(1)
        expect(store.getState().pendingNewSession).toBe(true)

        firstCreation.resolve({ sessionId: "shared-draft", startupInfo: null })
        await expect(Promise.all([automatic, explicit])).resolves.toEqual([
            "shared-draft",
            "shared-draft"
        ])
        expect(store.getState().activeSessionId).toBe("shared-draft")
        expect([...store.getState().sessions.keys()]).toEqual(["shared-draft"])
        expect(store.getState().sessions.get("shared-draft")).toMatchObject({
            ephemeral: true,
            transcript: []
        })
        expect(store.getState().pendingNewSession).toBe(false)
        expect(loadSessionIndex()).toEqual([])

        await expect(store.getState().newSession("/ws", "pi")).resolves.toBe("next-explicit")
        expect(connection.newSession).toHaveBeenCalledTimes(2)
        expect(store.getState().activeSessionId).toBe("next-explicit")
        expect([...store.getState().sessions.keys()]).toEqual(["next-explicit"])
        expect(store.getState().sessions.get("next-explicit")?.ephemeral).toBe(true)
        expect(loadSessionIndex()).toEqual([])
    })

    it("keeps returning the keyed auto-draft promise when an explicit winner appears mid-flight", async () => {
        const gate = deferred<{ sessionId: string; startupInfo: null }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(() => gate.promise)
        connection.dropSession = vi.fn()
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        const first = store.getState().ensureDraftSession("/ws", "pi")
        store.getState().upsertSessionMeta({ id: "explicit", cwd: "/ws" })
        store.getState().selectSession("explicit")
        const second = store.getState().ensureDraftSession("/ws/", "pi")

        expect(second).toBe(first)
        gate.resolve({ sessionId: "late-auto", startupInfo: null })
        await expect(first).resolves.toBe("explicit")
        expect(connection.dropSession).toHaveBeenCalledWith("late-auto")
        expect(store.getState().activeSessionId).toBe("explicit")
    })

    it("does not surface a late auto-draft failure after an explicit session wins", async () => {
        const gate = deferred<{ sessionId: string; startupInfo: null }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(() => gate.promise)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        const automatic = store.getState().ensureDraftSession("/ws", "pi")
        store.getState().upsertSessionMeta({ id: "explicit", cwd: "/ws" })
        store.getState().selectSession("explicit")
        gate.reject(new Error("late automatic failure"))

        await expect(automatic).rejects.toThrow("late automatic failure")
        expect(store.getState()).toMatchObject({
            activeSessionId: "explicit",
            connectionState: "ready",
            connectionError: null
        })
    })

    it("drops a late draft from an old workspace generation instead of making it active", async () => {
        const a = deferred<{ sessionId: string; startupInfo: null }>()
        const b = deferred<{ sessionId: string; startupInfo: null }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn((cwd) => cwd === "/ws-a" ? a.promise : b.promise)
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })

        store.getState().activateDraftWorkspace("/ws-a")
        const oldDraft = store.getState().ensureDraftSession("/ws-a", "pi")
        store.getState().activateDraftWorkspace("/ws-b")
        const currentDraft = store.getState().ensureDraftSession("/ws-b", "pi")

        a.resolve({ sessionId: "draft-a", startupInfo: null })
        await expect(oldDraft).resolves.toBeNull()
        expect(store.getState().sessions.has("draft-a")).toBe(false)
        expect(connection.dropSession).toHaveBeenCalledWith("draft-a")

        b.resolve({ sessionId: "draft-b", startupInfo: null })
        await expect(currentDraft).resolves.toBe("draft-b")
        expect(store.getState().activeSessionId).toBe("draft-b")
        await vi.waitFor(() => expect(connection.disposePrepared).toHaveBeenCalledWith("/ws-a"))
    })

    it("lets only the current workspace draft winner update the last-used curated agent", async () => {
        const oldWorkspace = deferred<{
            sessionId: string
            startupInfo: null
            agentIdentity: { selectedPreset: "pi"; commandMode: "latest"; trustedAgentId: "pi" }
        }>()
        const currentWorkspace = deferred<{
            sessionId: string
            startupInfo: null
            agentIdentity: { selectedPreset: "claude"; commandMode: "latest"; trustedAgentId: "claude" }
        }>()
        const connection = fakeConnection().connection
        connection.newSession = vi.fn((cwd) => cwd === "/ws-a"
            ? oldWorkspace.promise
            : currentWorkspace.promise)
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })

        store.getState().activateDraftWorkspace("/ws-a")
        const stale = store.getState().ensureDraftSession("/ws-a", "pi")
        store.getState().activateDraftWorkspace("/ws-b")
        const current = store.getState().ensureDraftSession("/ws-b", "claude")

        currentWorkspace.resolve({
            sessionId: "current-claude",
            startupInfo: null,
            agentIdentity: { selectedPreset: "claude", commandMode: "latest", trustedAgentId: "claude" }
        })
        await expect(current).resolves.toBe("current-claude")
        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBe("claude")

        oldWorkspace.resolve({
            sessionId: "stale-pi",
            startupInfo: null,
            agentIdentity: { selectedPreset: "pi", commandMode: "latest", trustedAgentId: "pi" }
        })
        await expect(stale).resolves.toBeNull()

        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBe("claude")
        expect(store.getState().sessions.has("stale-pi")).toBe(false)
        expect(connection.dropSession).toHaveBeenCalledWith("stale-pi")
    })

    it("keeps at most one unused draft across adapters and duplicate New actions, then promotes it", async () => {
        let counter = 0
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: `session-${++counter}`,
            startupInfo: null
        }))
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        await expect(store.getState().ensureDraftSession("/ws", "pi")).resolves.toBe("session-1")
        expect(loadSessionIndex()).toEqual([])

        await expect(store.getState().newSession("/ws", "claude")).resolves.toBe("session-2")
        await expect(store.getState().newSession("/ws", "codex")).resolves.toBe("session-3")
        await expect(store.getState().newSession("/ws", "pi")).resolves.toBe("session-4")

        expect(connection.newSession).toHaveBeenCalledTimes(4)
        expect(connection.dropSession).toHaveBeenCalledWith("session-1")
        expect(connection.dropSession).toHaveBeenCalledWith("session-2")
        expect(connection.dropSession).toHaveBeenCalledWith("session-3")
        expect(connection.disposePrepared).toHaveBeenCalledWith("/ws")
        expect(store.getState().sessions.has("session-1")).toBe(false)
        expect(store.getState().sessions.has("session-2")).toBe(false)
        expect(store.getState().sessions.has("session-3")).toBe(false)
        expect(store.getState().sessions.get("session-4")).toMatchObject({
            agentId: "pi",
            ephemeral: true,
            transcript: []
        })
        expect(loadSessionIndex()).toEqual([])

        await store.getState().sendPrompt("/ws", "first prompt")

        expect(store.getState().sessions.get("session-4")?.ephemeral).toBe(false)
        expect(loadSessionIndex()).toEqual([
            expect.objectContaining({ sessionId: "session-4", agentId: "pi", cwd: "/ws" })
        ])
    })

    it("replaces an ephemeral draft after pre-prompt adapter transcript callbacks", async () => {
        let counter = 0
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: `session-${++counter}`,
            startupInfo: null
        }))
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        await store.getState().newSession("/ws", "pi")
        store.getState().appendUpdate("session-1", {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "Adapter startup banner" }
        })
        store.getState().onSessionInfo("session-1", { queueDepth: 0, running: false })

        expect(store.getState().sessions.get("session-1")).toMatchObject({
            agentId: "pi",
            ephemeral: true,
            pendingTurn: false,
            running: false
        })
        expect(store.getState().sessions.get("session-1")?.transcript).not.toHaveLength(0)

        await expect(store.getState().newSession("/ws", "claude")).resolves.toBe("session-2")

        expect(connection.dropSession).toHaveBeenCalledWith("session-1")
        expect(connection.disposePrepared).toHaveBeenCalledWith("/ws")
        expect([...store.getState().sessions.keys()]).toEqual(["session-2"])
        expect(store.getState().sessions.get("session-2")).toMatchObject({
            agentId: "claude",
            ephemeral: true,
            transcript: []
        })
        expect(loadSessionIndex()).toEqual([])
    })

    it("disposes only the replaced draft's old router sub and preserves the explicit New owner", async () => {
        const stubs = new Map<string, AgentConnection>()
        const router = createAgentRouter({}, (command) => {
            const stub: AgentConnection = {
                newSession: vi.fn(async () => ({
                    sessionId: command.includes("codex") ? "explicit-codex" : "draft-pi",
                    startupInfo: null
                })),
                loadSession: vi.fn(),
                listSessions: vi.fn(async () => []),
                prompt: vi.fn(async () => "end_turn" as const),
                cancel: vi.fn(),
                disposePrepared: vi.fn(async () => true),
                dropSession: vi.fn()
            }
            stubs.set(command, stub)
            return stub
        })
        const store = createAgentStore({ connection: router })
        store.getState().activateDraftWorkspace("/ws")
        await store.getState().ensureDraftSession("/ws", "pi")
        store.getState().appendUpdate("draft-pi", {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "Adapter startup banner" }
        })
        store.getState().onSessionInfo("draft-pi", { queueDepth: 0, running: false })

        await expect(store.getState().newSession("/ws", "codex")).resolves.toBe("explicit-codex")

        await vi.waitFor(() => {
            expect(stubs.get("bunx pi-acp@latest")?.disposePrepared).toHaveBeenCalledWith("/ws")
        })
        expect(stubs.get("bunx pi-acp@latest")?.dropSession).toHaveBeenCalledWith("draft-pi")
        expect(stubs.get("bunx @agentclientprotocol/codex-acp@latest")?.disposePrepared).not.toHaveBeenCalled()
        expect([...store.getState().sessions.keys()]).toEqual(["explicit-codex"])
        expect(store.getState().sessions.get("explicit-codex")?.ephemeral).toBe(true)
    })

    it("keeps the inherited ephemeral flag when an explicit result reuses the same session id", async () => {
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({ sessionId: "same-id", startupInfo: null }))
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")
        await store.getState().ensureDraftSession("/ws", "pi")
        expect(store.getState().sessions.get("same-id")?.ephemeral).toBe(true)

        await store.getState().newSession("/ws", "pi")

        expect(store.getState().sessions.get("same-id")?.ephemeral).toBe(true)
        expect(loadSessionIndex()).toEqual([])
    })

    it("promotes an ephemeral draft and writes Session Index when the first prompt starts", async () => {
        const prompt = deferred<StopReason>()
        let counter = 0
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({ sessionId: `draft-${++counter}`, startupInfo: null }))
        connection.prompt = vi.fn(() => prompt.promise)
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")
        await store.getState().ensureDraftSession("/ws", "pi")

        const turn = store.getState().sendPrompt("/ws", "first prompt")

        expect(store.getState().sessions.get("draft-1")?.ephemeral).toBe(false)
        expect(loadSessionIndex()).toEqual([
            expect.objectContaining({ sessionId: "draft-1", cwd: "/ws" })
        ])
        prompt.resolve("end_turn")
        await turn
    })

    it("selects an existing restored session for cwd instead of creating a draft", async () => {
        const connection = fakeConnection().connection
        const store = createAgentStore({ connection })
        store.getState().hydrateRestoredSessions([{
            sessionId: "restored",
            cwd: "/ws",
            agentId: "pi",
            createdAt: 1,
            lastActiveAt: 1
        }])
        store.getState().activateDraftWorkspace("/ws")

        await expect(store.getState().ensureDraftSession("/ws", "pi")).resolves.toBe("restored")

        expect(connection.newSession).not.toHaveBeenCalled()
        expect(store.getState().activeSessionId).toBe("restored")
    })

    it("does not record failed or untrusted Custom session creation as last-used curated", async () => {
        const connection = fakeConnection().connection
        connection.newSession = vi.fn()
            .mockRejectedValueOnce(new Error("failed"))
            .mockResolvedValueOnce({
                sessionId: "custom",
                startupInfo: null,
                agentIdentity: { selectedPreset: "codex", commandMode: "custom", trustedAgentId: null }
            })
        const store = createAgentStore({ connection })

        await store.getState().newSession("/ws", "pi").catch(() => undefined)
        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBeNull()
        await store.getState().newSession("/ws", "codex")
        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBeNull()
    })

    it("drops an unexpectedly untrusted auto-draft result instead of making Custom visible", async () => {
        const connection = fakeConnection().connection
        connection.newSession = vi.fn(async () => ({
            sessionId: "unexpected-custom",
            startupInfo: null,
            agentIdentity: {
                selectedPreset: "pi" as const,
                commandMode: "custom" as const,
                trustedAgentId: null
            }
        }))
        connection.dropSession = vi.fn()
        connection.disposePrepared = vi.fn(async () => true)
        const store = createAgentStore({ connection })
        store.getState().activateDraftWorkspace("/ws")

        await expect(store.getState().ensureDraftSession("/ws", "pi")).resolves.toBeNull()

        expect(connection.dropSession).toHaveBeenCalledWith("unexpected-custom")
        expect(store.getState().sessions.size).toBe(0)
        expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBeNull()
    })
})

describe("Phase 4: hydrateRestoredSessions", () => {
    function indexEntry(overrides: Partial<SessionIndexEntry> = {}): SessionIndexEntry {
        return {
            sessionId: "restored-1",
            cwd: "/ws",
            agentId: "pi",
            agentTitle: "Fix login bug",
            sessionAlias: null,
            derivedTitle: "derived",
            createdAt: 1,
            lastActiveAt: 1,
            ...overrides
        }
    }

    it("builds idle, empty-transcript sessions marked restored, without touching activeSessionId", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })

        store.getState().hydrateRestoredSessions([indexEntry()])

        const session = store.getState().sessions.get("restored-1")
        expect(session).toMatchObject({
            cwd: "/ws",
            agentId: "pi",
            agentTitle: "Fix login bug",
            derivedTitle: "derived",
            tone: "idle",
            transcript: [],
            restored: true,
            title: "Fix login bug",
            configOptions: [],
            configRevision: 0,
            configRequest: null,
            configError: null
        })
        expect(session?.usage).toBeUndefined()
        expect(store.getState().activeSessionId).toBeNull()
    })

    it("does not clobber an already-known session with the same id", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        // replaceTranscript no longer creates ghost sessions (F3) — seed it first.
        store.getState().selectSession("restored-1")
        store.getState().replaceTranscript("restored-1", [{ id: "t0", who: "agent", text: "live", streaming: false }])

        store.getState().hydrateRestoredSessions([indexEntry()])

        expect(store.getState().sessions.get("restored-1")?.transcript).toEqual([
            { id: "t0", who: "agent", text: "live", streaming: false }
        ])
        expect(store.getState().sessions.get("restored-1")?.restored).toBeUndefined()
    })
})

describe("Phase 4: continueSession", () => {
    function restoredSession(overrides: Partial<SessionState> = {}): SessionState {
        return {
            title: "Fix login bug",
            agentId: "pi",
            agentLabel: "Agent",
            model: null,
            tone: "idle",
            transcript: [],
            availableCommands: [],
            stopReason: null,
            stopBadge: null,
            error: null,
            queueDepth: null,
            running: null,
            pendingTurn: false,
            metadataTitle: false,
            cwd: "/ws",
            restored: true,
            ...overrides
        }
    }

    it("a live session (restored not true) just selects it — no loadSession call", async () => {
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })
        store.getState().replaceTranscript("s-1", [])

        await store.getState().continueSession("s-1")

        expect(store.getState().activeSessionId).toBe("s-1")
        expect(fake.connection.loadSession).not.toHaveBeenCalled()
        expect(store.getState().composerFocusRequest).toEqual({ sessionId: "s-1", token: 1 })

        await store.getState().continueSession("s-1")
        expect(store.getState().composerFocusRequest).toEqual({ sessionId: "s-1", token: 2 })
    })

    it("a restored session with loadSession capability replays and clears the restored flag", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        vi.mocked(fake.connection.loadSession).mockImplementation(async () => {
            // 模擬 agent 在回應前先把歷史 replay 進來（透過 store 的 replaceTranscript，
            // 呼叫端在 AgentBridge 會接 onTranscript；這裡直接呼叫 store action 模擬）。
            store.getState().replaceTranscript("restored-1", [
                { id: "t0", who: "agent", text: "replayed history", streaming: false }
            ])
        })
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })
        upsertSessionIndexEntry({
            sessionId: "restored-1",
            cwd: "/ws",
            agentId: "pi",
            createdAt: 1,
            lastActiveAt: 1
        })

        await store.getState().continueSession("restored-1")

        // fixes F1：restored session 續聊要把 session.agentId 傳給連線，讓 unknown
        // sessionId 路由到正確的 sub connection，而非永遠落回全域預設 command。
        expect(fake.connection.supportsLoadSession).toHaveBeenCalledWith("/ws", "pi")
        expect(fake.connection.loadSession).toHaveBeenCalledWith("restored-1", "/ws", "pi")
        const session = store.getState().sessions.get("restored-1")
        expect(session?.restored).toBe(false)
        expect(session?.tone).toBe("idle")
        expect(session?.transcript).toEqual([
            { id: "t0", who: "agent", text: "replayed history", streaming: false }
        ])
        expect(store.getState().activeSessionId).toBe("restored-1")
        expect(loadSessionIndex()[0].lastActiveAt).toBeGreaterThanOrEqual(1)
    })

    it("fails closed during replay and replaces a restored trusted id with authoritative Custom", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const replay = deferred<{
            startupInfo: null
            agentIdentity: {
                selectedPreset: "codex"
                commandMode: "custom"
                trustedAgentId: null
            }
        }>()
        vi.mocked(fake.connection.loadSession).mockReturnValue(replay.promise)
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["restored-codex", restoredSession({ agentId: "codex" })]])
        })
        upsertSessionIndexEntry({
            sessionId: "restored-codex",
            cwd: "/ws",
            agentId: "codex",
            createdAt: 1,
            lastActiveAt: 1
        })

        const continuing = store.getState().continueSession("restored-codex")
        await vi.waitFor(() => expect(fake.connection.loadSession).toHaveBeenCalledOnce())
        expect(store.getState().sessions.get("restored-codex")?.agentId).toBeUndefined()

        replay.resolve({
            startupInfo: null,
            agentIdentity: {
                selectedPreset: "codex",
                commandMode: "custom",
                trustedAgentId: null
            }
        })
        await continuing

        expect(store.getState().sessions.get("restored-codex")?.agentId).toBe("custom")
        expect(loadSessionIndex()[0]).toMatchObject({
            sessionId: "restored-codex",
            agentId: "custom"
        })
    })

    it("keeps a restored trusted id when replay authoritatively verifies it", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        vi.mocked(fake.connection.loadSession).mockResolvedValue({
            startupInfo: null,
            agentIdentity: {
                selectedPreset: "codex",
                commandMode: "latest",
                trustedAgentId: "codex"
            }
        })
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["restored-codex", restoredSession({ agentId: "codex" })]])
        })

        await store.getState().continueSession("restored-codex")

        expect(store.getState().sessions.get("restored-codex")?.agentId).toBe("codex")
    })

    it("fails closed when a successful replay has no authoritative agent identity", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        vi.mocked(fake.connection.loadSession).mockResolvedValue({ startupInfo: null })
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["restored-codex", restoredSession({ agentId: "codex" })]])
        })

        await store.getState().continueSession("restored-codex")

        expect(store.getState().sessions.get("restored-codex")?.agentId).toBeUndefined()
    })

    it("a rapid double-click only triggers one loadSession call — fixes F4", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        const first = store.getState().continueSession("restored-1")
        const second = store.getState().continueSession("restored-1")
        await Promise.all([first, second])

        expect(fake.connection.loadSession).toHaveBeenCalledTimes(1)
    })

    it("keeps the sessionAlias after a successful replay — replay must not clobber the user's rename", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession({ sessionAlias: "My renamed chat", title: "My renamed chat" }))
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        expect(store.getState().sessions.get("restored-1")?.sessionAlias).toBe("My renamed chat")
        expect(store.getState().sessions.get("restored-1")?.title).toBe("My renamed chat")
    })

    it("sets infoBanner from loadSession's startupInfo result (feature detection)", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        vi.mocked(fake.connection.loadSession).mockResolvedValue({ startupInfo: "pi 0.0.31 · restored" })
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        expect(store.getState().sessions.get("restored-1")?.infoBanner).toBe("pi 0.0.31 · restored")
    })

    it("leaves infoBanner untouched when loadSession resolves without a startupInfo (void)", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        expect(store.getState().sessions.get("restored-1")?.infoBanner).toBeUndefined()
    })

    it("degrades with a notice block when the agent has not declared the loadSession capability", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => false)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        expect(fake.connection.loadSession).not.toHaveBeenCalled()
        const session = store.getState().sessions.get("restored-1")
        expect(session?.restored).toBe(true) // 保留舊條目，未被移除／清掉
        expect(session?.tone).toBe("idle")
        const notice = session?.transcript.at(-1)
        expect(notice && "kind" in notice ? notice.kind : null).toBe("notice")
        expect(notice && "actions" in notice ? notice.actions?.[0]?.kind : null).toBe("start_new_session")
    })

    it("degrades legacy custom entries without routing identity instead of using today's global command", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["legacy-custom", restoredSession({
                agentId: "custom",
                customCommandFingerprint: undefined
            })]])
        })

        await store.getState().continueSession("legacy-custom")

        expect(fake.connection.supportsLoadSession).not.toHaveBeenCalled()
        expect(fake.connection.loadSession).not.toHaveBeenCalled()
        const session = store.getState().sessions.get("legacy-custom")
        expect(session?.restored).toBe(true)
        expect(session?.transcript.at(-1)).toMatchObject({ kind: "notice" })
    })

    it("does not retain the replay guard after a legacy custom entry degrades", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["legacy-custom", restoredSession({
                agentId: "custom",
                customCommandFingerprint: undefined
            })]])
        })

        await store.getState().continueSession("legacy-custom")
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("legacy-custom", {
                ...sessions.get("legacy-custom")!,
                customCommandFingerprint: `sha256:${"b".repeat(64)}`
            })
            return { sessions }
        })
        await store.getState().continueSession("legacy-custom")

        expect(fake.connection.supportsLoadSession).toHaveBeenCalledOnce()
        expect(fake.connection.loadSession).toHaveBeenCalledOnce()
        expect(store.getState().sessions.get("legacy-custom")?.restored).toBe(false)
    })

    it("passes a restored custom command fingerprint through both replay preflights", async () => {
        const fingerprint = `sha256:${"a".repeat(64)}`
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        const store = createAgentStore({ connection: fake.connection })
        store.setState({
            sessions: new Map([["custom-1", restoredSession({
                agentId: "custom",
                customCommandFingerprint: fingerprint
            })]])
        })

        await store.getState().continueSession("custom-1")

        expect(fake.connection.supportsLoadSession)
            .toHaveBeenCalledWith("/ws", undefined, fingerprint)
        expect(fake.connection.loadSession)
            .toHaveBeenCalledWith("custom-1", "/ws", undefined, fingerprint)
    })

    it("does not spawn the current custom command when a restored fingerprint has drifted", async () => {
        const fingerprintA = await fingerprintAgentCommand("uvx agent-a --token private")
        expect(fingerprintA).toBeDefined()
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "custom",
            command: "uvx agent-b",
            traceEnabled: false
        }))
        const factory = vi.fn(() => fakeConnection().connection)
        const router = createAgentRouter({}, factory)
        const store = createAgentStore({ connection: router })
        store.setState({
            sessions: new Map([["custom-a", restoredSession({
                agentId: "custom",
                customCommandFingerprint: fingerprintA
            })]])
        })

        await store.getState().continueSession("custom-a")

        expect(factory).not.toHaveBeenCalled()
        expect(store.getState().sessions.get("custom-a")?.restored).toBe(true)
        expect(store.getState().sessions.get("custom-a")?.transcript.at(-1))
            .toMatchObject({ kind: "notice" })
    })

    it("clicking an unsupported restored row twice only leaves one notice — fixes F8", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => false)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        await store.getState().continueSession("restored-1")
        await store.getState().continueSession("restored-1")

        const transcript = store.getState().sessions.get("restored-1")?.transcript ?? []
        expect(transcript.filter((entry) => "kind" in entry && entry.kind === "notice")).toHaveLength(1)
    })

    it("degrades with a notice block when loadSession rejects", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => true)
        vi.mocked(fake.connection.loadSession).mockRejectedValue(new Error("session not found"))
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession())
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        const session = store.getState().sessions.get("restored-1")
        expect(session?.restored).toBe(true)
        const notice = session?.transcript.at(-1)
        expect(notice && "kind" in notice ? notice.kind : null).toBe("notice")
    })

    it("clicking the degrade notice's action starts a new session with the same cwd/agentId", async () => {
        const fake = fakeConnection()
        fake.connection.supportsLoadSession = vi.fn(async () => false)
        const store = createAgentStore({ connection: fake.connection })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-1", restoredSession({ agentId: "codex" }))
            return { sessions }
        })

        await store.getState().continueSession("restored-1")

        const notice = store.getState().sessions.get("restored-1")?.transcript.at(-1)
        const action = notice && "actions" in notice ? notice.actions?.[0] : undefined
        expect(action?.payload).toEqual({ cwd: "/ws", agentId: "codex" })
    })

    it("fixes F1 end-to-end: with global preset=pi, continuing a restored codex session spawns codex, not pi", async () => {
        // 全域 preset 保持預設 pi（localStorage 已在 beforeEach 清空），驗證真正的
        // agentRouter（而非 fakeConnection）在 unknown restored sessionId 時，依
        // session.agentId 路由到 codex 的 sub connection，而非落回全域預設的 pi。
        const spawnedCommands: string[] = []
        const factory = vi.fn((command: string) => {
            spawnedCommands.push(command)
            return {
                newSession: vi.fn(async () => ({ sessionId: "ignored", startupInfo: null })),
                loadSession: vi.fn(async () => ({ startupInfo: null })),
                listSessions: vi.fn(async () => []),
                prompt: vi.fn(async (): Promise<StopReason> => "end_turn"),
                cancel: vi.fn(),
                supportsLoadSession: vi.fn(async () => true)
            }
        })
        const router = createAgentRouter({}, factory)
        const store = createAgentStore({ connection: router })
        store.setState((state) => {
            const sessions = new Map(state.sessions)
            sessions.set("restored-codex", restoredSession({ agentId: "codex" }))
            return { sessions }
        })

        await store.getState().continueSession("restored-codex")

        expect(spawnedCommands).toEqual(["bunx @agentclientprotocol/codex-acp@latest"])
        expect(store.getState().sessions.get("restored-codex")?.restored).toBe(false)
    })
})

describe("Phase 5: Sessions context menu domain actions", () => {
    function plainSession(overrides: Partial<SessionState> = {}): SessionState {
        return {
            title: "New session",
            agentId: "pi",
            agentLabel: "Agent",
            model: null,
            tone: "idle",
            transcript: [],
            availableCommands: [],
            stopReason: null,
            stopBadge: null,
            error: null,
            queueDepth: null,
            running: null,
            pendingTurn: false,
            metadataTitle: false,
            cwd: "/ws",
            ...overrides
        }
    }

    describe("setSessionAlias", () => {
        it("trims and sets the alias, recomputing title, and syncs the Session Index", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({ derivedTitle: "first prompt" }))
                return { sessions }
            })
            upsertSessionIndexEntry({
                sessionId: "s-1",
                cwd: "/ws",
                createdAt: 1,
                lastActiveAt: 1,
                derivedTitle: "first prompt"
            })

            store.getState().setSessionAlias("s-1", "  My chat  ")

            expect(store.getState().sessions.get("s-1")).toMatchObject({
                sessionAlias: "My chat",
                title: "My chat"
            })
            expect(loadSessionIndex()[0]).toMatchObject({ sessionAlias: "My chat" })
        })

        it("an empty/whitespace alias clears it, falling back to agentTitle then derivedTitle", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({
                    sessionAlias: "Renamed",
                    agentTitle: "Fix login bug",
                    derivedTitle: "first prompt",
                    title: "Renamed"
                }))
                return { sessions }
            })

            store.getState().setSessionAlias("s-1", "   ")

            expect(store.getState().sessions.get("s-1")).toMatchObject({
                sessionAlias: null,
                title: "Fix login bug"
            })
        })

        it("is a silent no-op for a session that does not exist", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })

            expect(() => store.getState().setSessionAlias("ghost", "name")).not.toThrow()
            expect(store.getState().sessions.has("ghost")).toBe(false)
        })
    })

    describe("removeSession", () => {
        it("removes a non-pending session without sending cancel, and removes its Session Index entry", async () => {
            const fake = fakeConnection()
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession())
                return { sessions }
            })
            upsertSessionIndexEntry({ sessionId: "s-1", cwd: "/ws", createdAt: 1, lastActiveAt: 1 })

            await expect(store.getState().removeSession("s-1")).resolves.toBe(true)

            expect(store.getState().sessions.has("s-1")).toBe(false)
            expect(loadSessionIndex()).toEqual([])
            expect(fake.connection.cancel).not.toHaveBeenCalled()
        })

        it("tells the connection to drop the session's runtime state — fixes F10", () => {
            const fake = fakeConnection()
            fake.connection.dropSession = vi.fn()
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession())
                return { sessions }
            })

            store.getState().removeSession("s-1")

            expect(fake.connection.dropSession).toHaveBeenCalledWith("s-1")
        })

        it("awaits a successful cancel before removing an in-flight session", async () => {
            const fake = fakeConnection()
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({ pendingTurn: true, running: true }))
                return { sessions }
            })

            await expect(store.getState().removeSession("s-1")).resolves.toBe(true)

            expect(fake.connection.cancel).toHaveBeenCalledWith("s-1")
            expect(store.getState().sessions.has("s-1")).toBe(false)
        })

        it("keeps an in-flight session and its index when cancel fails", async () => {
            const fake = fakeConnection()
            vi.mocked(fake.connection.cancel).mockRejectedValueOnce(new Error("cancel refused"))
            fake.connection.dropSession = vi.fn()
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({ pendingTurn: true, running: true }))
                return { sessions }
            })
            upsertSessionIndexEntry({ sessionId: "s-1", cwd: "/ws", createdAt: 1, lastActiveAt: 1 })

            await expect(store.getState().removeSession("s-1")).rejects.toThrow("cancel refused")

            expect(store.getState().sessions.get("s-1")).toMatchObject({
                pendingTurn: true,
                running: true
            })
            expect(loadSessionIndex()).toHaveLength(1)
            expect(fake.connection.dropSession).not.toHaveBeenCalled()
        })

        it("switches the active session to the same-cwd session with the most recent lastActiveAt", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({ cwd: "/ws" }))
                sessions.set("s-older", plainSession({ cwd: "/ws" }))
                sessions.set("s-newer", plainSession({ cwd: "/ws" }))
                sessions.set("s-other-cwd", plainSession({ cwd: "/other" }))
                return { sessions, activeSessionId: "s-1" }
            })
            upsertSessionIndexEntry({ sessionId: "s-older", cwd: "/ws", createdAt: 1, lastActiveAt: 10 })
            upsertSessionIndexEntry({ sessionId: "s-newer", cwd: "/ws", createdAt: 1, lastActiveAt: 20 })

            store.getState().removeSession("s-1")

            expect(store.getState().activeSessionId).toBe("s-newer")
        })

        it("falls back to null when removing the active session leaves no same-cwd sessions", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession({ cwd: "/ws" }))
                return { sessions, activeSessionId: "s-1" }
            })

            store.getState().removeSession("s-1")

            expect(store.getState().activeSessionId).toBeNull()
        })

        it("leaves an unrelated activeSessionId untouched", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession())
                sessions.set("s-2", plainSession())
                return { sessions, activeSessionId: "s-2" }
            })

            store.getState().removeSession("s-1")

            expect(store.getState().activeSessionId).toBe("s-2")
        })

        it("is a stale-safe no-op for a session that does not exist", async () => {
            const store = createAgentStore({ connection: fakeConnection().connection })

            await expect(store.getState().removeSession("ghost")).resolves.toBe(false)
        })
    })

    describe("F3: in-flight session removal must not resurrect a ghost", () => {
        it("sendPrompt's resolve tail does not recreate a session removed mid-flight", async () => {
            const fake = fakeConnection()
            fake.holdPrompt()
            const store = createAgentStore({ connection: fake.connection })
            const turn = store.getState().sendPrompt("/ws", "hi")
            await vi.waitFor(() => expect(store.getState().sessions.has("s-1")).toBe(true))

            await store.getState().removeSession("s-1")
            fake.promptGate.resolve("end_turn")
            await turn

            expect(store.getState().sessions.has("s-1")).toBe(false)
        })

        it("sendPrompt's catch tail does not recreate a session removed mid-flight", async () => {
            const fake = fakeConnection()
            fake.holdPrompt()
            const store = createAgentStore({ connection: fake.connection })
            const turn = store.getState().sendPrompt("/ws", "hi").catch(() => undefined)
            await vi.waitFor(() => expect(store.getState().sessions.has("s-1")).toBe(true))

            await store.getState().removeSession("s-1")
            fake.promptGate.reject(new Error("boom"))
            await turn

            expect(store.getState().sessions.has("s-1")).toBe(false)
        })

        it("replaceTranscript ignores a late update for a session that no longer exists", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })

            store.getState().replaceTranscript("removed-1", [
                { id: "t0", who: "agent", text: "late replay", streaming: false }
            ])

            expect(store.getState().sessions.has("removed-1")).toBe(false)
        })

        it("drops all queued runtime callbacks after an existing session is removed", async () => {
            const fake = fakeConnection()
            fake.connection.dropSession = vi.fn()
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("removed-1", plainSession({ cwd: "/ws" }))
                return { sessions }
            })

            await store.getState().removeSession("removed-1")
            store.getState().setAvailableCommands("removed-1", [{ name: "late", description: "Late" }])
            store.getState().appendUpdate("removed-1", {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "late" }
            })
            store.getState().replaceTranscript("removed-1", [
                { id: "t0", who: "agent", text: "late replay", streaming: false }
            ])
            store.getState().onSessionInfo("removed-1", { queueDepth: 1, running: true })
            store.getState().onPermissionRequest("removed-1", permissionBlock, vi.fn())
            store.getState().markConnectionError("removed-1", new Error("late disconnect"))
            store.getState().upsertSessionMeta({ id: "removed-1", cwd: "/ws", name: "late meta" })

            expect(store.getState().sessions.has("removed-1")).toBe(false)
            expect(store.getState().pendingPermissions.has("removed-1")).toBe(false)
            expect(store.getState()).toMatchObject({
                connectionState: "ready",
                connectionError: null
            })
        })

        it("does not let an older listSessions result clear a removed-session tombstone", async () => {
            const listed = deferred<Array<{ id: string; cwd: string; name: string }>>()
            const fake = fakeConnection()
            fake.connection.listSessions = vi.fn(() => listed.promise)
            const store = createAgentStore({ connection: fake.connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("removed-1", plainSession({ cwd: "/ws" }))
                return { sessions }
            })

            const loading = store.getState().loadSessions("/ws")
            await store.getState().removeSession("removed-1")
            listed.resolve([{ id: "removed-1", cwd: "/ws", name: "stale list result" }])
            await loading

            expect(store.getState().sessions.has("removed-1")).toBe(false)
        })
    })

    describe("rename channel", () => {
        it("beginRenameSession sets renamingSessionId only for an existing session", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState((state) => {
                const sessions = new Map(state.sessions)
                sessions.set("s-1", plainSession())
                return { sessions }
            })

            store.getState().beginRenameSession("ghost")
            expect(store.getState().renamingSessionId).toBeNull()

            store.getState().beginRenameSession("s-1")
            expect(store.getState().renamingSessionId).toBe("s-1")
        })

        it("endRenameSession clears the channel", () => {
            const store = createAgentStore({ connection: fakeConnection().connection })
            store.setState({ renamingSessionId: "s-1" })

            store.getState().endRenameSession()

            expect(store.getState().renamingSessionId).toBeNull()
        })
    })

    describe("remove-confirm channel", () => {
        it("requestRemoveSessionConfirm resolves true/false once respondRemoveSessionConfirm is called", async () => {
            const store = createAgentStore({ connection: fakeConnection().connection })

            const pending = store.getState().requestRemoveSessionConfirm("s-1")
            expect(store.getState().confirmRemoveRequest).toMatchObject({ sessionId: "s-1" })
            store.getState().respondRemoveSessionConfirm(true)

            await expect(pending).resolves.toBe(true)
            expect(store.getState().confirmRemoveRequest).toBeNull()
        })

        it("a second request resolves the still-pending previous one with false", async () => {
            const store = createAgentStore({ connection: fakeConnection().connection })

            const first = store.getState().requestRemoveSessionConfirm("s-1")
            const second = store.getState().requestRemoveSessionConfirm("s-2")
            store.getState().respondRemoveSessionConfirm(true)

            await expect(first).resolves.toBe(false)
            await expect(second).resolves.toBe(true)
        })
    })
})

describe("selectWorkspaceAgentCounts", () => {
    const emptyForTest: SessionState = {
        title: "New session",
        agentLabel: "Agent",
        model: null,
        tone: "idle",
        transcript: [],
        availableCommands: [],
        stopReason: null,
        stopBadge: null,
        error: null,
        queueDepth: null,
        running: null,
        pendingTurn: false,
        metadataTitle: false,
        cwd: null
    }

    it("groups sessions by normalized cwd with total and running counts", () => {
        const sessions = new Map<string, SessionState>([
            ["a", { ...emptyForTest, cwd: "/ws-a", running: true }],
            ["b", { ...emptyForTest, cwd: "/ws-a", tone: "run" }],
            ["c", { ...emptyForTest, cwd: "/ws-b" }],
            ["d", { ...emptyForTest, cwd: null }]
        ])
        const counts = selectWorkspaceAgentCounts(sessions)
        expect(counts.get("/ws-a")).toEqual({ total: 2, running: 2 })
        expect(counts.get("/ws-b")).toEqual({ total: 1, running: 0 })
        expect(counts.size).toBe(2)
    })
})

describe("P3: form elicitation queue", () => {
    const request = (message: string): ElicitationRequest => ({
        message,
        fields: [{
            key: "choice",
            type: "string",
            required: true,
            options: [{ value: "red", label: "Red" }, { value: "blue", label: "Blue" }]
        }]
    })

    it("queues elicitations per session without overwriting earlier resolvers", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const first = vi.fn()
        const second = vi.fn()
        store.getState().onElicitationRequest("s-1", request("first"), first)
        store.getState().onElicitationRequest("s-1", request("second"), second)

        const queue = store.getState().pendingElicitations.get("s-1") ?? []
        expect(queue).toHaveLength(2)
        expect(queue[0].request.message).toBe("first")
        expect(queue[1].request.message).toBe("second")
        expect(store.getState().sessions.get("s-1")?.tone).toBe("wait")
    })

    it("responds by id, keeps wait tone while queued, resumes run when drained", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const first = vi.fn()
        const second = vi.fn()
        store.getState().onElicitationRequest("s-1", request("first"), first)
        store.getState().onElicitationRequest("s-1", request("second"), second)
        const queue = store.getState().pendingElicitations.get("s-1") ?? []

        store.getState().respondElicitation("s-1", queue[0].id, {
            action: "accept",
            content: { choice: "red" }
        })
        expect(first).toHaveBeenCalledExactlyOnceWith({ action: "accept", content: { choice: "red" } })
        expect(store.getState().pendingElicitations.get("s-1")).toHaveLength(1)
        expect(store.getState().sessions.get("s-1")?.tone).toBe("wait")

        const remaining = store.getState().pendingElicitations.get("s-1") ?? []
        store.getState().respondElicitation("s-1", remaining[0].id, { action: "cancel" })
        expect(second).toHaveBeenCalledExactlyOnceWith({ action: "cancel" })
        expect(store.getState().pendingElicitations.has("s-1")).toBe(false)
        expect(store.getState().sessions.get("s-1")?.tone).toBe("run")
    })

    it("ignores stale ids and clears the queue when the session is cancelled", async () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const respond = vi.fn()
        store.getState().onElicitationRequest("s-1", request("only"), respond)

        store.getState().respondElicitation("s-1", "el-unknown", { action: "cancel" })
        expect(respond).not.toHaveBeenCalled()

        await store.getState().cancel("s-1")
        // wire resolver 的取消由連線層 cancelPendingPermissions 負責；store 只清 UI state。
        expect(store.getState().pendingElicitations.has("s-1")).toBe(false)
    })
})
