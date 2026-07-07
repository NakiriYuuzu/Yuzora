import { beforeEach, describe, expect, it, vi } from "vitest"

import { AgentAuthRequiredError, type AgentConnection, type AgentAuthMethod, type StopReason } from "@/agent/acpConnection"
import type { BlockEntry, TranscriptEntry } from "@/agent/acpTypes"
import { createAgentStore } from "./agentStore"

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
        newSession: vi.fn(async () => "s-1"),
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

beforeEach(() => {
    vi.restoreAllMocks()
})

const permissionBlock: BlockEntry = {
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
    it("records session/new auth-required state and clears it after retry creates a session", async () => {
        const fake = fakeConnection()
        vi.mocked(fake.connection.newSession)
            .mockRejectedValueOnce(new AgentAuthRequiredError({
                authMethods: [terminalAuthMethod],
                cwd: "/w",
                sessionId: null
            }))
            .mockResolvedValueOnce("s-after-login")
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

    it("marks connection/process prompt errors as fail without losing the transcript", async () => {
        const fake = fakeConnection()
        fake.rejectPrompt(new Error("ACP agent exited"))
        const store = createAgentStore({ connection: fake.connection })

        await expect(store.getState().sendPrompt("/w", "Continue")).rejects.toThrow("ACP agent exited")

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("fail")
        expect(session?.error).toBe("ACP agent exited")
        expect(session?.transcript).toEqual([{ who: "you", text: "Continue", streaming: false }])
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

    it("clears pending permission state when cancelling a session", () => {
        const choose = vi.fn()
        const fake = fakeConnection()
        const store = createAgentStore({ connection: fake.connection })

        store.getState().replaceTranscript("s-1", [permissionBlock])
        store.getState().onPermissionRequest("s-1", permissionBlock, choose)
        store.getState().cancel("s-1")

        expect(fake.connection.cancel).toHaveBeenCalledExactlyOnceWith("s-1")
        expect(choose).not.toHaveBeenCalled()
        expect(store.getState().pendingPermissions.has("s-1")).toBe(false)
        expect(store.getState().sessions.get("s-1")).toMatchObject({
            tone: "idle",
            stopReason: "cancelled",
            error: null
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
            { who: "you", text: "First prompt should not replace metadata title", streaming: true }
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

    it("does not let running=false session info overwrite a failed session", () => {
        const store = createAgentStore({ connection: fakeConnection().connection })
        const transcript: TranscriptEntry[] = [{ who: "agent", text: "partial", streaming: true }]

        store.getState().replaceTranscript("s-1", transcript)
        store.getState().markConnectionError("s-1", new Error("process died"))
        store.getState().onSessionInfo("s-1", { queueDepth: 0, running: false })

        const session = store.getState().sessions.get("s-1")
        expect(session?.tone).toBe("fail")
        expect(session?.queueDepth).toBe(0)
        expect(session?.running).toBe(false)
    })
})
