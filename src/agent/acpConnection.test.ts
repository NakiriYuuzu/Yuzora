import { afterEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { emit } from "@tauri-apps/api/event"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import {
    createAcpClientRuntime,
    createAcpConnection,
    isAgentAuthRequiredError,
    reduceSessionUpdate
} from "./acpConnection"
import type { BlockEntry, TranscriptEntry } from "./acpTypes"
import { createFakeAcpAgentBridge } from "./fakeAcpAgent"
import { dropDocument, getDocument } from "../editor/documentRegistry"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { handleExternalChange } from "../lib/externalChange"
import { recentlySaved } from "../lib/saveSuppress"

const registeredViews: { path: string; view: EditorView }[] = []

afterEach(() => {
    for (const { path, view } of registeredViews.splice(0)) {
        unregisterView(path, view)
        view.destroy()
        dropDocument(path)
    }
    vi.restoreAllMocks()
    clearMocks()
})

function mountView(path: string, doc: string): EditorView {
    const view = new EditorView({ state: EditorState.create({ doc }) })
    registerView(path, view)
    registeredViews.push({ path, view })
    return view
}

function withTestTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<T>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error("timed out waiting for promise")), ms)
    })
    promise.catch(() => {})
    return Promise.race([promise, timeoutPromise]).finally(() => {
        if (timeout) clearTimeout(timeout)
    })
}

async function waitForFakeMethod(fake: ReturnType<typeof createFakeAcpAgentBridge>, method: string): Promise<void> {
    await vi.waitFor(() => {
        expect(fake.messages.some((message) => message.method === method)).toBe(true)
    })
}

function createAuthRequiredFakeAcpAgentBridge(
    emitLine: (line: string) => Promise<void>,
    authOn: "session/new" | "session/prompt"
) {
    let buffer = ""
    const messages: Array<{ id?: string | number | null; method?: string; params?: Record<string, unknown> }> = []
    const authMethods = [{
        id: "pi_terminal_login",
        name: "Launch pi in the terminal",
        description: "Start pi in an interactive terminal to configure API keys or login",
        type: "terminal",
        args: ["--terminal-login"],
        env: {}
    }]

    async function write(chunk: string) {
        buffer += chunk
        const lines = buffer.split("\n")
        buffer = lines.pop() ?? ""
        for (const line of lines) {
            const trimmed = line.trim()
            if (!trimmed) continue
            const message = JSON.parse(trimmed) as { id?: string | number | null; method?: string; params?: Record<string, unknown> }
            messages.push(message)

            if (message.method === "initialize") {
                await emitLine(JSON.stringify({
                    jsonrpc: "2.0",
                    id: message.id,
                    result: {
                        protocolVersion: 1,
                        agentCapabilities: { loadSession: true },
                        authMethods
                    }
                }))
            }
            if (message.method === "session/new") {
                await emitLine(JSON.stringify(authOn === "session/new"
                    ? {
                        jsonrpc: "2.0",
                        id: message.id,
                        error: { code: -32000, message: "Authentication required" }
                    }
                    : {
                        jsonrpc: "2.0",
                        id: message.id,
                        result: { sessionId: "fake-session" }
                    }))
            }
            if (message.method === "session/prompt") {
                await emitLine(JSON.stringify({
                    jsonrpc: "2.0",
                    id: message.id,
                    error: { code: -32000, message: "Authentication required" }
                }))
            }
        }
    }

    return { messages, write }
}

describe("reduceSessionUpdate", () => {
    it("merges agent chunks and updates a tool call by toolCallId", () => {
        let t: TranscriptEntry[] = []
        t = reduceSessionUpdate(t, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Hel" } })
        t = reduceSessionUpdate(t, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "lo" } })
        t = reduceSessionUpdate(t, { sessionUpdate: "tool_call", toolCallId: "tc1", title: "edit a.rs", status: "pending" })
        t = reduceSessionUpdate(t, { sessionUpdate: "tool_call_update", toolCallId: "tc1", status: "completed", content: [{ type: "diff", path: "a.rs", oldText: "x", newText: "y" }] })
        expect(t[0]).toMatchObject({ who: "agent", text: "Hello" })
        const tool = t.find((e) => "kind" in e && e.kind !== "diff")
        expect(tool).toMatchObject({ kind: "tool", meta: expect.stringContaining("completed") })
        expect(t.some((e) => "kind" in e && e.kind === "diff")).toBe(true)
    })

    it("covers session/update discriminants and content placeholders", () => {
        let t: TranscriptEntry[] = []
        t = reduceSessionUpdate(t, { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Hi" } })
        expect(t).toEqual([{ who: "you", text: "Hi", streaming: true }])

        expect(reduceSessionUpdate(t, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "hidden" } })).toBe(t)
        expect(reduceSessionUpdate(t, { sessionUpdate: "session_info_update" })).toBe(t)
        expect(reduceSessionUpdate(t, { sessionUpdate: "current_mode_update", currentModeId: "plan" })).toBe(t)

        t = reduceSessionUpdate(t, {
            sessionUpdate: "plan",
            entries: [{ content: "Inspect", priority: "high", status: "pending" }]
        })
        t = reduceSessionUpdate(t, {
            sessionUpdate: "plan",
            entries: [{ content: "Inspect", priority: "high", status: "completed" }]
        })
        const plans = t.filter((entry) => "kind" in entry && entry.kind === "plan")
        expect(plans).toHaveLength(1)
        expect(plans[0]).toMatchObject({ text: "[x] Inspect" })

        t = reduceSessionUpdate(t, { sessionUpdate: "agent_message_chunk", content: { type: "image" } })
        t = reduceSessionUpdate(t, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "resource_link", title: "Spec", uri: "file:///spec.md" }
        })
        t = reduceSessionUpdate(t, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "resource", resource: { uri: "file:///embedded.md", text: "Embedded resource" } }
        })
        expect(t.some((entry) => "who" in entry && entry.text.includes("[image]Spec"))).toBe(true)
        expect(t.some((entry) => "who" in entry && entry.text.includes("Embedded resource"))).toBe(true)

        t = reduceSessionUpdate(t, {
            sessionUpdate: "tool_call_update",
            toolCallId: "missing-tool",
            status: "completed",
            content: [{ type: "content", content: { type: "resource_link", name: "Log", uri: "file:///log" } }]
        })
        expect(t.some((entry) => "kind" in entry && entry.kind === "tool" && entry.text.includes("Log"))).toBe(true)
    })
})

describe("requestPermission", () => {
    it("maps options to a perm block, resolves the selected option, remembers allow_always by tool kind, and cancels pending requests", async () => {
        const permissionBlocks: BlockEntry[] = []
        const runtime = createAcpClientRuntime({
            onPermissionRequest: (_sessionId, block, choose) => {
                permissionBlocks.push(block)
                if (permissionBlocks.length === 1) choose("always")
            }
        })

        const first = await runtime.client.requestPermission({
            sessionId: "s1",
            toolCall: {
                toolCallId: "opaque|id",
                title: "write file",
                kind: "edit",
                status: "pending"
            },
            options: [
                { optionId: "once", name: "Allow once", kind: "allow_once" },
                { optionId: "always", name: "Always allow edits", kind: "allow_always" },
                { optionId: "reject", name: "Reject", kind: "reject_once" }
            ]
        })

        expect(first).toEqual({ outcome: { outcome: "selected", optionId: "always" } })
        expect(permissionBlocks[0]).toMatchObject({
            kind: "perm",
            text: expect.stringContaining("write file")
        })
        expect(permissionBlocks[0].actions).toEqual([
            expect.objectContaining({ label: "Allow once", kind: "allow_once" }),
            expect.objectContaining({ label: "Always allow edits", kind: "allow_always" }),
            expect.objectContaining({ label: "Reject", kind: "reject_once" })
        ])

        const second = await runtime.client.requestPermission({
            sessionId: "s1",
            toolCall: {
                toolCallId: "another|opaque",
                title: "edit again",
                kind: "edit"
            },
            options: [
                { optionId: "always2", name: "Always allow edits", kind: "allow_always" },
                { optionId: "reject2", name: "Reject", kind: "reject_once" }
            ]
        })
        expect(second).toEqual({ outcome: { outcome: "selected", optionId: "always2" } })
        expect(permissionBlocks).toHaveLength(1)

        let cancelChooser: ((optionId: string) => void) | undefined
        const cancelRuntime = createAcpClientRuntime({
            onPermissionRequest: (_sessionId, _block, choose) => {
                cancelChooser = choose
            }
        })
        const pending = cancelRuntime.client.requestPermission({
            sessionId: "s-cancel",
            toolCall: { toolCallId: "cmd|1", title: "run command", kind: "execute" },
            options: [{ optionId: "run", name: "Run", kind: "allow_once" }]
        })
        expect(cancelChooser).toBeTypeOf("function")
        cancelRuntime.cancelPendingPermissions("s-cancel")
        cancelChooser?.("run")
        await expect(pending).resolves.toEqual({ outcome: { outcome: "cancelled" } })
    })

    it("does not auto-select allow_always for a different tool kind", async () => {
        const onPermissionRequest = vi.fn((_sessionId, _block: BlockEntry, choose: (optionId: string) => void) => {
            choose(onPermissionRequest.mock.calls.length === 1 ? "always-edit" : "allow-run")
        })
        const runtime = createAcpClientRuntime({ onPermissionRequest })

        await runtime.client.requestPermission({
            sessionId: "s1",
            toolCall: { toolCallId: "edit|1", title: "edit", kind: "edit" },
            options: [{ optionId: "always-edit", name: "Always", kind: "allow_always" }]
        })
        const execute = await runtime.client.requestPermission({
            sessionId: "s1",
            toolCall: { toolCallId: "exec|1", title: "run", kind: "execute" },
            options: [{ optionId: "allow-run", name: "Run", kind: "allow_once" }]
        })

        expect(execute).toEqual({ outcome: { outcome: "selected", optionId: "allow-run" } })
        expect(onPermissionRequest).toHaveBeenCalledTimes(2)
    })
})

describe("fs callbacks", () => {
    it("writes an open document through CodeMirror, saves it, and marks the path recently saved", async () => {
        const path = "/w/open.ts"
        const events: unknown[] = []
        const mark = vi.spyOn(recentlySaved, "mark").mockImplementation((markedPath) => {
            events.push(["mark", markedPath])
        })
        const calls: unknown[] = []
        mockIPC((cmd, payload) => {
            calls.push([cmd, payload])
            if (cmd === "open_file") return { kind: "full", content: "old", size: 3 }
            if (cmd === "save_file") return 123
            return undefined
        })
        await getDocument(path)
        const view = mountView(path, "old")
        const runtime = createAcpClientRuntime()

        await runtime.client.writeTextFile({
            sessionId: "s1",
            path,
            content: "new"
        })

        expect(view.state.doc.toString()).toBe("new")
        expect((await getDocument(path)).result).toMatchObject({ kind: "full", content: "new" })
        expect(calls).toContainEqual(["save_file", { path, content: "new" }])
        expect(mark).toHaveBeenCalledWith(path)
        expect(events).toEqual([["mark", path]])
        expect(handleExternalChange(
            [path],
            [{ path, name: "open.ts", dirty: false, externallyModified: false }],
            new Set(mark.mock.calls.map(([markedPath]) => markedPath))
        )).toEqual({ reload: [], markModified: [] })
    })

    it("writes unopened files through save_file IPC and marks the path recently saved", async () => {
        const path = "/w/unopened.ts"
        const events: unknown[] = []
        const mark = vi.spyOn(recentlySaved, "mark").mockImplementation((markedPath) => {
            events.push(["mark", markedPath])
        })
        const calls: unknown[] = []
        mockIPC((cmd, payload) => {
            calls.push([cmd, payload])
            if (cmd === "open_file") throw new Error("writeTextFile should not open unopened files")
            if (cmd === "save_file") return 124
            return undefined
        })
        const runtime = createAcpClientRuntime()

        await runtime.client.writeTextFile({
            sessionId: "s1",
            path,
            content: "disk"
        })

        expect(calls).toContainEqual(["save_file", { path, content: "disk" }])
        expect(mark).toHaveBeenCalledWith(path)
        expect(events).toEqual([["mark", path]])
        expect(handleExternalChange(
            [path],
            [{ path, name: "unopened.ts", dirty: false, externallyModified: false }],
            new Set(mark.mock.calls.map(([markedPath]) => markedPath))
        )).toEqual({ reload: [], markModified: [] })
    })

    it("reads dirty open buffer content first and honors line and limit", async () => {
        const path = "/w/dirty.ts"
        mountView(path, "one\ntwo\nthree\nfour")
        mockIPC((cmd) => {
            if (cmd === "open_file") return { kind: "full", content: "disk", size: 4 }
            return undefined
        })
        const runtime = createAcpClientRuntime()

        const result = await runtime.client.readTextFile({
            sessionId: "s1",
            path,
            line: 2,
            limit: 2
        })

        expect(result).toEqual({ content: "two\nthree" })
    })

    it("handles readTextFile line and limit boundaries", async () => {
        const path = "/w/bounds.ts"
        mountView(path, "one\ntwo\nthree")
        const runtime = createAcpClientRuntime()

        await expect(runtime.client.readTextFile({ sessionId: "s1", path, line: 1, limit: 1 }))
            .resolves.toEqual({ content: "one" })
        await expect(runtime.client.readTextFile({ sessionId: "s1", path, line: 99, limit: 1 }))
            .resolves.toEqual({ content: "" })
        await expect(runtime.client.readTextFile({ sessionId: "s1", path, line: 2, limit: 0 }))
            .resolves.toEqual({ content: "" })
        await expect(runtime.client.readTextFile({ sessionId: "s1", path }))
            .resolves.toEqual({ content: "one\ntwo\nthree" })
    })
})

describe("terminal callbacks", () => {
    it("proxies terminal methods to IPC and appends terminal output to a tool block", async () => {
        const calls: unknown[] = []
        const transcriptSnapshots: TranscriptEntry[][] = []
        mockIPC((cmd, payload) => {
            calls.push([cmd, payload])
            if (cmd === "agent_terminal_create") return "term-1"
            if (cmd === "agent_terminal_output") {
                return {
                    output: "hello\n",
                    truncated: false,
                    exit_status: { exit_code: 0, signal: null }
                }
            }
            if (cmd === "agent_terminal_wait_for_exit") return { exit_code: 0, signal: null }
            return undefined
        })
        const runtime = createAcpClientRuntime({
            onTranscript: (_sessionId, transcript) => transcriptSnapshots.push(transcript)
        })

        await expect(runtime.client.createTerminal({
            sessionId: "s1",
            command: "sh",
            args: ["-c", "printf hello"],
            env: [{ name: "A", value: "B" }],
            cwd: "/w"
        })).resolves.toEqual({ terminalId: "term-1" })
        await expect(runtime.client.terminalOutput({
            sessionId: "s1",
            terminalId: "term-1"
        })).resolves.toEqual({
            output: "hello\n",
            truncated: false,
            exitStatus: { exitCode: 0, signal: null }
        })
        await expect(runtime.client.waitForTerminalExit({
            sessionId: "s1",
            terminalId: "term-1"
        })).resolves.toEqual({ exitCode: 0, signal: null })
        await runtime.client.killTerminal({ sessionId: "s1", terminalId: "term-1" })
        await runtime.client.releaseTerminal({ sessionId: "s1", terminalId: "term-1" })

        expect(calls).toContainEqual(["agent_terminal_create", {
            command: "sh",
            args: ["-c", "printf hello"],
            env: [["A", "B"]],
            cwd: "/w",
            byteLimit: 1_048_576
        }])
        expect(calls).toContainEqual(["agent_terminal_output", { id: "term-1" }])
        expect(calls).toContainEqual(["agent_terminal_wait_for_exit", { id: "term-1" }])
        expect(calls).toContainEqual(["agent_terminal_kill", { id: "term-1" }])
        expect(calls).toContainEqual(["agent_terminal_release", { id: "term-1" }])
        expect(transcriptSnapshots.at(-1)?.at(-1)).toMatchObject({
            kind: "tool",
            text: "hello\n"
        })
    })
})

describe("createAcpConnection", () => {
    it("normalizes session/new auth-required errors with initialize auth methods", async () => {
        const agentId = "agent-auth"
        const fake = createAuthRequiredFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            "session/new"
        )

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-auth-agent",
            initializeTimeoutMs: 1_000
        })

        let error: unknown
        try {
            await connection.newSession("/w")
        } catch (caught) {
            error = caught
        }

        expect(isAgentAuthRequiredError(error)).toBe(true)
        if (!isAgentAuthRequiredError(error)) throw new Error("expected auth-required error")
        expect(error.cwd).toBe("/w")
        expect(error.sessionId).toBeNull()
        expect(error.authMethods).toEqual([{
            id: "pi_terminal_login",
            name: "Launch pi in the terminal",
            description: "Start pi in an interactive terminal to configure API keys or login",
            type: "terminal",
            args: ["--terminal-login"],
            env: {}
        }])
    })

    it("normalizes prompt auth-required errors with the session id", async () => {
        const agentId = "agent-auth-prompt"
        const fake = createAuthRequiredFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            "session/prompt"
        )

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-auth-agent",
            initializeTimeoutMs: 1_000
        })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        let error: unknown
        try {
            await connection.prompt("fake-session", [{ type: "text", text: "hello" }])
        } catch (caught) {
            error = caught
        }

        expect(isAgentAuthRequiredError(error)).toBe(true)
        if (!isAgentAuthRequiredError(error)) throw new Error("expected auth-required error")
        expect(error.sessionId).toBe("fake-session")
        expect(error.cwd).toBe("/w")
        expect(error.authMethods[0]).toMatchObject({
            id: "pi_terminal_login",
            type: "terminal",
            args: ["--terminal-login"]
        })
    })

    it("maps a -32602 invalid-cwd session/new error to a friendly message", async () => {
        const agentId = "agent-cwd"
        let buffer = ""
        const emitLine = (line: string) => emit("agent://stdout", { id: agentId, line })
        async function write(chunk: string) {
            buffer += chunk
            const lines = buffer.split("\n")
            buffer = lines.pop() ?? ""
            for (const line of lines) {
                const trimmed = line.trim()
                if (!trimmed) continue
                const message = JSON.parse(trimmed) as { id?: string | number | null; method?: string }
                if (message.method === "initialize") {
                    await emitLine(JSON.stringify({
                        jsonrpc: "2.0",
                        id: message.id,
                        result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
                    }))
                }
                if (message.method === "session/new") {
                    await emitLine(JSON.stringify({
                        jsonrpc: "2.0",
                        id: message.id,
                        error: { code: -32602, message: "cwd must be an absolute path" }
                    }))
                }
            }
        }

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return write((payload as { chunk: string }).chunk)
            if (cmd === "agent_stderr_tail") return []
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-cwd-agent",
            initializeTimeoutMs: 1_000
        })

        let error: unknown
        try {
            await connection.newSession(".")
        } catch (caught) {
            error = caught
        }

        expect(error).toBeInstanceOf(Error)
        expect((error as Error).message).toMatch(/absolute path/i)
        expect((error as Error).message).toContain(".")
        expect(isAgentAuthRequiredError(error)).toBe(false)
    })

    it("drives initialize, newSession, prompt, permission, session_info_update, and edit diff through the SDK bridge", async () => {
        const agentId = "agent-1"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const ipcCalls: unknown[] = []
        const permissionBlocks: BlockEntry[] = []
        const transcriptSnapshots: TranscriptEntry[][] = []
        const sessionInfo: unknown[] = []
        const commands: unknown[] = []

        mockIPC((cmd, payload) => {
            ipcCalls.push([cmd, payload])
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") {
                return fake.write((payload as { chunk: string }).chunk)
            }
            if (cmd === "agent_kill") return undefined
            if (cmd === "agent_list") throw new Error("agent_list must not be used for session listing")
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onPermissionRequest: (_sessionId, block, choose) => {
                permissionBlocks.push(block)
                choose("allow")
            },
            onTranscript: (_sessionId, transcript) => transcriptSnapshots.push(transcript),
            onSessionInfo: (sessionId, info) => sessionInfo.push([sessionId, info]),
            onAvailableCommands: (sessionId, availableCommands) => commands.push([sessionId, availableCommands])
        })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        await expect(connection.prompt("fake-session", [{ type: "text", text: "edit hello" }])).resolves.toBe("end_turn")
        await expect(connection.listSessions("/w")).resolves.toEqual([{ id: "fake-session", cwd: "/w" }])
        await expect(connection.loadSession("loaded-session", "/other")).resolves.toBeUndefined()
        await expect(connection.listSessions("/w")).resolves.toEqual([{ id: "fake-session", cwd: "/w" }])
        await expect(connection.listSessions("/other")).resolves.toEqual([{ id: "loaded-session", cwd: "/other" }])

        expect(ipcCalls).toContainEqual(["agent_spawn", { command: "fake-acp-agent", cwd: "/w" }])
        expect(fake.messages.some((message) =>
            message.method === "session/new"
            && Array.isArray(message.params?.mcpServers)
            && message.params.mcpServers.length === 0
        )).toBe(true)
        expect(permissionBlocks[0]).toMatchObject({ kind: "perm", text: "Edit file" })
        expect(sessionInfo).toContainEqual(["fake-session", { queueDepth: 0, running: true }])
        expect(commands).toContainEqual(["fake-session", [{ name: "fix", description: "Run fake fix" }]])

        const finalTranscript = transcriptSnapshots.at(-1) ?? []
        expect(finalTranscript.some((entry) => "who" in entry && entry.who === "agent" && entry.text.includes("Ready"))).toBe(true)
        expect(finalTranscript.some((entry) => "kind" in entry && entry.kind === "diff" && entry.text === "hello.txt")).toBe(true)
    })

    it("rejects prompt before initialize", async () => {
        const ipcCalls: unknown[] = []
        mockIPC((cmd, payload) => {
            ipcCalls.push([cmd, payload])
            return undefined
        }, { shouldMockEvents: true })
        const connection = createAcpConnection({ command: "fake-acp-agent" })

        await expect(connection.prompt("missing-session", [{ type: "text", text: "hello" }]))
            .rejects.toThrow("ACP connection is not initialized")
        expect(ipcCalls).toEqual([])
    })

    it("rejects newSession promptly (not after the timeout) when the agent exits before initialize responds", async () => {
        const agentId = "agent-exit-before-init"
        const ipcCalls: Array<[string, unknown]> = []
        mockIPC((cmd, payload) => {
            ipcCalls.push([cmd, payload])
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return undefined
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "nonexistent-cmd",
            initializeTimeoutMs: 5_000
        })

        const newSession = withTestTimeout(connection.newSession("/w"), 200)
        await vi.waitFor(() => {
            expect(ipcCalls.some(([cmd, payload]) =>
                cmd === "agent_write"
                && (payload as { chunk: string }).chunk.includes("\"method\":\"initialize\"")
            )).toBe(true)
        })

        const rejection = expect(newSession).rejects.toThrow(/agent 行程已結束（exit code 127）/)
        await emit("agent://exit", { id: agentId, code: 127 })
        await rejection
    })

    it("rejects an in-flight prompt when the agent exits", async () => {
        const agentId = "agent-1"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const permissionBlocks: BlockEntry[] = []

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onPermissionRequest: (_sessionId, block) => {
                permissionBlocks.push(block)
            }
        })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        const prompt = withTestTimeout(
            connection.prompt("fake-session", [{ type: "text", text: "wait for exit" }]),
            100
        )
        await waitForFakeMethod(fake, "session/prompt")
        expect(permissionBlocks).toHaveLength(1)

        const rejection = expect(prompt).rejects.toThrow("ACP agent exited")
        await emit("agent://exit", { id: agentId, code: 0 })
        await rejection
    })

    it("does not create an unhandled rejection when the agent exits after a prompt completes", async () => {
        const proc = (globalThis as unknown as {
            process: {
                on(ev: string, cb: (e: unknown) => void): void
                off(ev: string, cb: (e: unknown) => void): void
            }
        }).process
        const agentId = "agent-1"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const rejections: unknown[] = []
        const onUnhandled = (error: unknown) => rejections.push(error)
        proc.on("unhandledRejection", onUnhandled)

        try {
            mockIPC((cmd, payload) => {
                if (cmd === "agent_spawn") return agentId
                if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
                if (cmd === "agent_kill") return undefined
                return undefined
            }, { shouldMockEvents: true })

            const connection = createAcpConnection({
                command: "fake-acp-agent",
                initializeTimeoutMs: 1_000,
                onPermissionRequest: (_sessionId, _block, choose) => choose("allow")
            })

            await expect(connection.newSession("/w")).resolves.toBe("fake-session")
            await expect(connection.prompt("fake-session", [{ type: "text", text: "complete before exit" }]))
                .resolves.toBe("end_turn")
            await emit("agent://exit", { id: agentId, code: 0 })
            await new Promise((resolve) => setTimeout(resolve, 10))

            expect(rejections).toEqual([])
        } finally {
            proc.off("unhandledRejection", onUnhandled)
        }
    })

    it("resets stale connection state on agent exit and can spawn again", async () => {
        let spawnCount = 0
        const fakes = new Map<string, ReturnType<typeof createFakeAcpAgentBridge>>()
        const exited = new Set<string>()

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") {
                spawnCount += 1
                const id = `agent-${spawnCount}`
                fakes.set(id, createFakeAcpAgentBridge((line) => emit("agent://stdout", { id, line })))
                return id
            }
            if (cmd === "agent_write") {
                const { id, chunk } = payload as { id: string; chunk: string }
                if (exited.has(id)) throw new Error("write to exited process")
                const fake = fakes.get(id)
                if (!fake) throw new Error(`unknown fake agent ${id}`)
                return fake.write(chunk)
            }
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onPermissionRequest: (_sessionId, _block, choose) => choose("allow")
        })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        exited.add("agent-1")
        await emit("agent://exit", { id: "agent-1", code: 0 })

        await expect(withTestTimeout(
            connection.prompt("fake-session", [{ type: "text", text: "after exit" }]),
            100
        ))
            .rejects.toThrow("ACP agent exited")
        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        expect(spawnCount).toBe(2)
        await expect(connection.prompt("fake-session", [{ type: "text", text: "after respawn" }]))
            .resolves.toBe("end_turn")
    })

    it("defaults the initialize timeout to 15s", async () => {
        vi.useFakeTimers()
        try {
            const agentId = "agent-default-timeout"
            mockIPC((cmd) => {
                if (cmd === "agent_spawn") return agentId
                if (cmd === "agent_write") return undefined
                if (cmd === "agent_stderr_tail") return []
                if (cmd === "agent_kill") return undefined
                return undefined
            }, { shouldMockEvents: true })

            const connection = createAcpConnection({ command: "hang-agent" })
            const newSession = connection.newSession("/w")
            const assertion = expect(newSession).rejects.toThrow(/timed out after 15000ms/)
            await vi.advanceTimersByTimeAsync(15_000)
            await assertion
        } finally {
            vi.useRealTimers()
        }
    })

    it("kills with init_timeout after fetching the stderr tail when initialize times out", async () => {
        vi.useFakeTimers()
        try {
            const agentId = "agent-init-timeout"
            const calls: Array<[string, unknown]> = []
            mockIPC((cmd, payload) => {
                calls.push([cmd, payload])
                if (cmd === "agent_spawn") return agentId
                if (cmd === "agent_write") return undefined
                if (cmd === "agent_stderr_tail") return ["pi: fatal: could not connect"]
                if (cmd === "agent_kill") return undefined
                return undefined
            }, { shouldMockEvents: true })

            const connection = createAcpConnection({
                command: "hang-agent",
                initializeTimeoutMs: 15_000
            })
            const newSession = connection.newSession("/w")
            const assertion = expect(newSession).rejects.toThrow(/see Settings → Logs \(source: acp\)/)
            await vi.advanceTimersByTimeAsync(15_000)
            await assertion

            const killIndex = calls.findIndex(([cmd]) => cmd === "agent_kill")
            const tailIndex = calls.findIndex(([cmd]) => cmd === "agent_stderr_tail")
            expect(tailIndex).toBeGreaterThanOrEqual(0)
            expect(killIndex).toBeGreaterThan(tailIndex)
            expect(calls[killIndex]?.[1]).toEqual({ id: agentId, reason: "init_timeout" })
            expect(calls[tailIndex]?.[1]).toEqual({ id: agentId })
        } finally {
            vi.useRealTimers()
        }
    })

    it("rejects initialize immediately when agent_write fails, without waiting for the timeout", async () => {
        const agentId = "agent-write-fail"
        mockIPC((cmd) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") throw new Error("write to closed pipe")
            if (cmd === "agent_stderr_tail") return []
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "broken-agent",
            initializeTimeoutMs: 60_000
        })

        await withTestTimeout(
            expect(connection.newSession("/w")).rejects.toThrow(/write to closed pipe/),
            200
        )
    })

    it("surfaces an EPIPE agent crash from the exit stderr tail", async () => {
        const agentId = "agent-epipe"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const permissionBlocks: BlockEntry[] = []

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            if (cmd === "agent_stderr_tail") return []
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onPermissionRequest: (_sessionId, block) => {
                permissionBlocks.push(block)
            }
        })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        const prompt = withTestTimeout(
            connection.prompt("fake-session", [{ type: "text", text: "wait for crash" }]),
            100
        )
        await waitForFakeMethod(fake, "session/prompt")
        expect(permissionBlocks).toHaveLength(1)

        const rejection = expect(prompt).rejects.toThrow(/agent adapter crashed \(EPIPE\)/)
        await emit("agent://exit", { id: agentId, code: 1, stderrTail: ["node: write EPIPE"] })
        await rejection
    })

    it("does not double-process agent output on reconnect after an initialize timeout", async () => {
        // regression：timeout 路徑先清 agentProcessId 再 kill，若 listener guard 讀共享
        // 可變 id，舊 listener 永不 unlisten，且會把下一條連線的 stdout 重複 enqueue
        //（同一行 JSON-RPC 處理兩次 → transcript 變 "ReadyReady"）。
        let spawnCount = 0
        const fakes = new Map<string, ReturnType<typeof createFakeAcpAgentBridge>>()
        const transcriptSnapshots: TranscriptEntry[][] = []

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") {
                spawnCount += 1
                const id = `agent-${spawnCount}`
                fakes.set(id, createFakeAcpAgentBridge((line) => emit("agent://stdout", { id, line })))
                return id
            }
            if (cmd === "agent_write") {
                const { id, chunk } = payload as { id: string; chunk: string }
                if (id === "agent-1") return undefined // 第一個 agent 永不回應 → initialize timeout
                return fakes.get(id)?.write(chunk)
            }
            if (cmd === "agent_stderr_tail") return []
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 50,
            onPermissionRequest: (_sessionId, _block, choose) => choose("allow"),
            onTranscript: (_sessionId, transcript) => transcriptSnapshots.push(transcript)
        })

        await expect(connection.newSession("/w")).rejects.toThrow(/timed out/)
        // kill 後 Rust 會 emit 舊 agent 的 exit——不得波及下一條連線的全域狀態
        await emit("agent://exit", { id: "agent-1", code: null, stderrTail: [] })

        await expect(connection.newSession("/w")).resolves.toBe("fake-session")
        expect(spawnCount).toBe(2)
        await expect(connection.prompt("fake-session", [{ type: "text", text: "hi" }]))
            .resolves.toBe("end_turn")

        const finalTranscript = transcriptSnapshots.at(-1) ?? []
        const agentTexts = finalTranscript
            .filter((entry): entry is TranscriptEntry & { who: string; text: string } =>
                "who" in entry && entry.who === "agent")
            .map((entry) => entry.text)
        expect(agentTexts.some((text) => text.includes("Ready"))).toBe(true)
        expect(agentTexts.some((text) => text.includes("ReadyReady"))).toBe(false)
    })
})
