import { afterEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { emit } from "@tauri-apps/api/event"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import {
    createAcpClientRuntime,
    createAcpConnection,
    isAgentAuthRequiredError,
    normalizeSessionConfigOptions,
    reduceSessionUpdate
} from "./acpConnection"
import type { BlockEntry, TranscriptEntry } from "./acpTypes"
import { createFakeAcpAgentBridge, PINNED_AGENT_COMMAND_FIXTURES } from "./fakeAcpAgent"
import { documentGeneration, dropDocument, getDocument } from "../editor/documentRegistry"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { handleExternalChange } from "../lib/externalChange"
import { recentlySaved } from "../lib/saveSuppress"
import { useWorkspaceStore } from "../state/workspaceStore"

const registeredViews: { path: string; view: EditorView }[] = []

afterEach(() => {
    for (const { path, view } of registeredViews.splice(0)) {
        unregisterView(path, view)
        view.destroy()
        dropDocument(path)
        useWorkspaceStore.getState().closeTabsByPath([path])
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

    it("user_message_chunk 的 image content（replay 場景）轉入 MsgEntry.images 而非文字佔位", () => {
        let t: TranscriptEntry[] = []
        t = reduceSessionUpdate(t, { sessionUpdate: "user_message_chunk", content: { type: "text", text: "look:" } })
        t = reduceSessionUpdate(t, {
            sessionUpdate: "user_message_chunk",
            content: { type: "image", data: "aGVsbG8=", mimeType: "image/png" }
        })
        expect(t).toHaveLength(1)
        expect(t[0]).toMatchObject({
            who: "you",
            text: "look:",
            images: [{ mimeType: "image/png", dataUrl: "data:image/png;base64,aGVsbG8=" }]
        })

        // 沒有前導文字時 image chunk 自建 user entry；缺 data/mimeType 落回文字佔位。
        let solo: TranscriptEntry[] = []
        solo = reduceSessionUpdate(solo, {
            sessionUpdate: "user_message_chunk",
            content: { type: "image", data: "eA==", mimeType: "image/webp" }
        })
        expect(solo[0]).toMatchObject({ who: "you", text: "", images: [{ mimeType: "image/webp" }] })

        let malformed: TranscriptEntry[] = []
        malformed = reduceSessionUpdate(malformed, {
            sessionUpdate: "user_message_chunk",
            content: { type: "image" }
        })
        expect(malformed[0]).toMatchObject({ who: "you", text: "[image]" })
    })

    it("covers session/update discriminants and content placeholders", () => {
        let t: TranscriptEntry[] = []
        t = reduceSessionUpdate(t, { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Hi" } })
        expect(t).toEqual([{ who: "you", text: "Hi", streaming: true }])

        const withThought = reduceSessionUpdate(t, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "hidden" } })
        expect(withThought.at(-1)).toMatchObject({ kind: "thought", text: "hidden" })
        const merged = reduceSessionUpdate(withThought, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: " more" } })
        expect(merged.at(-1)).toMatchObject({ kind: "thought", text: "hidden more" })
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

    it("treats usage_update and config_option_update as no-ops that never touch the transcript", () => {
        const t: TranscriptEntry[] = [{ who: "agent", text: "hi", streaming: false }]
        expect(reduceSessionUpdate(t, {
            sessionUpdate: "usage_update",
            used: 120,
            size: 1000,
            cost: { amount: 0.01, currency: "USD" }
        })).toBe(t)
        expect(reduceSessionUpdate(t, {
            sessionUpdate: "config_option_update",
            configOptions: []
        })).toBe(t)
    })

    it("silently ignores a truly unknown session update variant and warns instead of rendering an error block", () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
        const t: TranscriptEntry[] = [{ who: "agent", text: "hi", streaming: false }]

        const result = reduceSessionUpdate(t, { sessionUpdate: "some_future_variant", foo: "bar" })

        expect(result).toBe(t)
        expect(result.some((entry) => "kind" in entry && entry.kind === "error")).toBe(false)
        expect(warn).toHaveBeenCalledWith(expect.stringContaining("some_future_variant"))
    })

    it("warns only once per unknown session update variant, even across multiple occurrences", () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
        const t: TranscriptEntry[] = []

        reduceSessionUpdate(t, { sessionUpdate: "another_future_variant" })
        reduceSessionUpdate(t, { sessionUpdate: "another_future_variant" })

        const callsForVariant = warn.mock.calls.filter((args) =>
            typeof args[0] === "string" && args[0].includes("another_future_variant")
        )
        expect(callsForVariant).toHaveLength(1)
    })

    it("still renders an error block when tool_call/tool_call_update payloads are malformed (regression)", () => {
        const withBadToolCall = reduceSessionUpdate([], { sessionUpdate: "tool_call", title: "no id" })
        expect(withBadToolCall.at(-1)).toMatchObject({
            kind: "error",
            text: expect.stringContaining("tool_call")
        })

        const withBadToolCallUpdate = reduceSessionUpdate([], { sessionUpdate: "tool_call_update", status: "completed" })
        expect(withBadToolCallUpdate.at(-1)).toMatchObject({
            kind: "error",
            text: expect.stringContaining("tool_call_update")
        })
    })
})

describe("session config normalization", () => {
    it("preserves grouped select, boolean, category, currentValue, and raw metadata", () => {
        expect(normalizeSessionConfigOptions([
            {
                id: "model",
                name: "Model",
                description: null,
                category: "model",
                type: "select",
                currentValue: "fast",
                options: [{
                    group: "recommended",
                    name: "Recommended",
                    options: [{
                        value: "fast",
                        name: "Fast",
                        description: "Quick",
                        _meta: { raw: "option" }
                    }],
                    _meta: { raw: "group" }
                }],
                _meta: { raw: "config" }
            },
            {
                id: "auto",
                name: "Auto",
                category: "_custom",
                type: "boolean",
                currentValue: true
            }
        ])).toEqual([
            {
                id: "model",
                name: "Model",
                description: null,
                category: "model",
                type: "select",
                currentValue: "fast",
                options: [{
                    group: "recommended",
                    name: "Recommended",
                    options: [{
                        value: "fast",
                        name: "Fast",
                        description: "Quick",
                        _meta: { raw: "option" }
                    }],
                    _meta: { raw: "group" }
                }],
                _meta: { raw: "config" }
            },
            {
                id: "auto",
                name: "Auto",
                category: "_custom",
                type: "boolean",
                currentValue: true
            }
        ])
    })
})

describe("recordUserPrompt (P3)", () => {
  it("seeds the user block so a later agent chunk keeps the you prefix", async () => {
    const seen: TranscriptEntry[][] = []
    const runtime = createAcpClientRuntime({ onTranscript: (_id, t) => seen.push(t) })
    runtime.recordUserPrompt("s1", [{ type: "text", text: "hello" }])
    expect(seen).toHaveLength(0) // 刻意不觸發 onTranscript
    await runtime.client.sessionUpdate({
      sessionId: "s1",
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hi" } },
    } as never)
    expect(seen.at(-1)).toEqual([
      { who: "you", text: "hello", streaming: true },
      { who: "agent", text: "hi", streaming: true },
    ])
  })

  it("settles previous turn's streaming entries before appending the next user turn", async () => {
    const seen: TranscriptEntry[][] = []
    const runtime = createAcpClientRuntime({ onTranscript: (_id, t) => seen.push(t) })
    // Turn 1：agent 回覆到一半就中斷（連線層的 transcripts 仍停在 streaming:true）。
    await runtime.client.sessionUpdate({
      sessionId: "s1",
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "turn1 reply" } },
    } as never)
    // Turn 2 開始：recordUserPrompt 應先把 turn1 的殘留 streaming 收斂。
    runtime.recordUserPrompt("s1", [{ type: "text", text: "turn2" }])
    await runtime.client.sessionUpdate({
      sessionId: "s1",
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "turn2 reply" } },
    } as never)
    const last = seen.at(-1)!
    expect(last).toEqual([
      { who: "agent", text: "turn1 reply", streaming: false },
      { who: "you", text: "turn2", streaming: true },
      { who: "agent", text: "turn2 reply", streaming: true },
    ])
  })
})

describe("dropSession (F10)", () => {
  it("clears the session's transcript so a later update starts fresh instead of appending to old history", async () => {
    const seen: TranscriptEntry[][] = []
    const runtime = createAcpClientRuntime({ onTranscript: (_id, t) => seen.push(t) })
    await runtime.client.sessionUpdate({
      sessionId: "s1",
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "old history" } },
    } as never)
    expect(seen.at(-1)).toEqual([{ who: "agent", text: "old history", streaming: true }])

    runtime.dropSession("s1")

    await runtime.client.sessionUpdate({
      sessionId: "s1",
      update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "fresh" } },
    } as never)
    // dropSession 已清掉 transcripts.get("s1")，所以 reduceSessionUpdate 是從空陣列
    // 開始累加，不再帶著已移除 session 的舊 "old history"。
    expect(seen.at(-1)).toEqual([{ who: "agent", text: "fresh", streaming: true }])
  })
})

describe("usage and title side-effect dispatch", () => {
    it("dispatches onUsage with used/size/cost parsed from a usage_update notification", async () => {
        const usages: unknown[] = []
        const runtime = createAcpClientRuntime({ onUsage: (sessionId, usage) => usages.push([sessionId, usage]) })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: 120, size: 1000, cost: { amount: 0.01, currency: "USD" } }
        } as never)

        expect(usages).toEqual([["s1", { used: 120, size: 1000, cost: { amount: 0.01, currency: "USD" } }]])
    })

    it("ignores usage_update when used/size are not numbers, and tolerates a missing cost", async () => {
        const usages: unknown[] = []
        const runtime = createAcpClientRuntime({ onUsage: (sessionId, usage) => usages.push([sessionId, usage]) })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: "not-a-number", size: 1000 }
        } as never)
        expect(usages).toEqual([])

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: 5, size: 10 }
        } as never)
        expect(usages).toEqual([["s1", { used: 5, size: 10 }]])
    })

    it("ignores usage_update when used/size is NaN or Infinity", async () => {
        const usages: unknown[] = []
        const runtime = createAcpClientRuntime({ onUsage: (sessionId, usage) => usages.push([sessionId, usage]) })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: NaN, size: 1000 }
        } as never)
        expect(usages).toEqual([])

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: 120, size: Infinity }
        } as never)
        expect(usages).toEqual([])
    })

    it("drops cost when cost.amount is NaN but still reports used/size", async () => {
        const usages: unknown[] = []
        const runtime = createAcpClientRuntime({ onUsage: (sessionId, usage) => usages.push([sessionId, usage]) })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "usage_update", used: 120, size: 1000, cost: { amount: NaN, currency: "USD" } }
        } as never)

        expect(usages).toEqual([["s1", { used: 120, size: 1000 }]])
    })

    it("dispatches onSessionTitle for a non-empty official title, alongside onSessionInfo for _meta.piAcp", async () => {
        const titles: unknown[] = []
        const sessionInfo: unknown[] = []
        const runtime = createAcpClientRuntime({
            onSessionTitle: (sessionId, title) => titles.push([sessionId, title]),
            onSessionInfo: (sessionId, info) => sessionInfo.push([sessionId, info])
        })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: {
                sessionUpdate: "session_info_update",
                title: "Fix login bug",
                _meta: { piAcp: { queueDepth: 1, running: true } }
            }
        } as never)

        expect(titles).toEqual([["s1", "Fix login bug"]])
        expect(sessionInfo).toEqual([["s1", { queueDepth: 1, running: true }]])
    })

    it("does not dispatch onSessionTitle when title is absent, while onSessionInfo still fires", async () => {
        const titles: unknown[] = []
        const sessionInfo: unknown[] = []
        const runtime = createAcpClientRuntime({
            onSessionTitle: (sessionId, title) => titles.push([sessionId, title]),
            onSessionInfo: (sessionId, info) => sessionInfo.push([sessionId, info])
        })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: { sessionUpdate: "session_info_update", _meta: { piAcp: { queueDepth: 0, running: false } } }
        } as never)

        expect(titles).toEqual([])
        expect(sessionInfo).toEqual([["s1", { queueDepth: 0, running: false }]])
    })

    it("routes config_option_update through the authoritative side-effect callback", async () => {
        const updates: unknown[] = []
        const runtime = createAcpClientRuntime({
            onConfigOptions: (sessionId, configOptions) => updates.push([sessionId, configOptions])
        })

        await runtime.client.sessionUpdate({
            sessionId: "s1",
            update: {
                sessionUpdate: "config_option_update",
                configOptions: [{
                    id: "model",
                    name: "Model",
                    category: "model",
                    type: "select",
                    currentValue: "fast",
                    options: [{ value: "fast", name: "Fast" }]
                }]
            }
        })

        expect(updates).toEqual([["s1", [{
            id: "model",
            name: "Model",
            category: "model",
            type: "select",
            currentValue: "fast",
            options: [{ value: "fast", name: "Fast" }]
        }]]])
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
            if (cmd === "open_file") return { kind: "full", content: "old", size: 3, lineEnding: "lf" }
            if (cmd === "save_file") return 123
            return undefined
        })
        await getDocument(path)
        const view = mountView(path, "old")
        useWorkspaceStore.getState().openTab(path)
        useWorkspaceStore.getState().hydrateLineEnding(path, "lf", documentGeneration(path))
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

    it("preserves CRLF on ACP writes to an open normalized document", async () => {
        const path = "/w/open-crlf.ts"
        const calls: unknown[] = []
        mockIPC((cmd, payload) => {
            calls.push([cmd, payload])
            if (cmd === "save_file") return 123
            return undefined
        })
        const view = mountView(path, "old\nline\n")
        const store = useWorkspaceStore.getState()
        store.openTab(path)
        store.hydrateLineEnding(path, "crlf", documentGeneration(path))
        const runtime = createAcpClientRuntime()

        await runtime.client.writeTextFile({
            sessionId: "s1",
            path,
            content: "new\nline\n"
        })

        expect(view.state.doc.toString()).toBe("new\nline\n")
        expect(calls).toContainEqual(["save_file", { path, content: "new\r\nline\r\n" }])
    })

    it("blocks ACP writes to an open Mixed document before buffer mutation or I/O", async () => {
        const path = "/w/open-mixed.ts"
        const calls: unknown[] = []
        const mark = vi.spyOn(recentlySaved, "mark")
        mockIPC((cmd, payload) => {
            calls.push([cmd, payload])
            return undefined
        })
        const view = mountView(path, "old\nline\n")
        const store = useWorkspaceStore.getState()
        store.openTab(path)
        store.hydrateLineEnding(path, "mixed", documentGeneration(path))
        const runtime = createAcpClientRuntime()

        await expect(runtime.client.writeTextFile({
            sessionId: "s1",
            path,
            content: "new\nline\n"
        })).rejects.toThrow("Mixed")

        expect(view.state.doc.toString()).toBe("old\nline\n")
        expect(mark).not.toHaveBeenCalled()
        expect(calls.some((entry) => Array.isArray(entry) && entry[0] === "save_file")).toBe(false)
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        await expect(connection.prompt("fake-session", [{ type: "text", text: "edit hello" }])).resolves.toBe("end_turn")
        await expect(connection.listSessions("/w")).resolves.toEqual([{ id: "fake-session", cwd: "/w" }])
        await expect(connection.loadSession("loaded-session", "/other")).resolves.toMatchObject({
            startupInfo: null,
            configOptions: expect.any(Array)
        })
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

    it("carries usage_update and titled session_info_update through the SDK bridge to onUsage/onSessionTitle", async () => {
        const agentId = "agent-usage"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const usages: unknown[] = []
        const titles: unknown[] = []

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onUsage: (sessionId, usage) => usages.push([sessionId, usage]),
            onSessionTitle: (sessionId, title) => titles.push([sessionId, title])
        })

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        await fake.emitSessionUpdate({ sessionUpdate: "usage_update", used: 42, size: 500 })
        await fake.emitSessionUpdate({ sessionUpdate: "session_info_update", title: "Fix login bug" })

        expect(usages).toEqual([["fake-session", { used: 42, size: 500 }]])
        expect(titles).toEqual([["fake-session", "Fix login bug"]])
    })

    it("returns initial configOptions from new/load and setter responses as full authoritative sets", async () => {
        const agentId = "agent-config"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        const created = await connection.newSession("/w")
        expect(created.configOptions).toHaveLength(3)
        expect(created.configOptions).toEqual(expect.arrayContaining([
            expect.objectContaining({ id: "model", type: "select", category: "model", currentValue: "fake-fast" }),
            expect.objectContaining({ id: "auto-execute", type: "boolean", currentValue: true })
        ]))

        const updated = await connection.setSessionConfigOption?.("fake-session", "model", "fake-reasoning")
        expect(updated).toHaveLength(3)
        expect(updated).toEqual(expect.arrayContaining([
            expect.objectContaining({ id: "model", currentValue: "fake-reasoning" }),
            expect.objectContaining({
                id: "effort",
                currentValue: "high",
                options: [
                    { value: "medium", name: "Medium" },
                    { value: "high", name: "High" }
                ]
            })
        ]))

        const booleanUpdated = await connection.setSessionConfigOption?.("fake-session", "auto-execute", false)
        expect(booleanUpdated).toEqual(expect.arrayContaining([
            expect.objectContaining({ id: "auto-execute", type: "boolean", currentValue: false })
        ]))
        expect(fake.messages).toContainEqual(expect.objectContaining({
            method: "session/set_config_option",
            params: { sessionId: "fake-session", configId: "auto-execute", type: "boolean", value: false }
        }))

        const loaded = await connection.loadSession("fake-session", "/w")
        expect(loaded && "configOptions" in loaded ? loaded.configOptions : undefined).toHaveLength(3)
    })

    it("rejects active/pending setters and keeps a newer notification over a late setter response", async () => {
        const agentId = "agent-config-race"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { deferConfigSet: true }
        )
        const notifications: unknown[] = []
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onConfigOptions: (_sessionId, configOptions) => notifications.push(configOptions)
        })
        await connection.newSession("/w")

        await fake.emitSessionUpdate({
            sessionUpdate: "session_info_update",
            _meta: { piAcp: { queueDepth: 0, running: true } }
        })
        await expect(
            connection.setSessionConfigOption?.("fake-session", "model", "fake-reasoning")
        ).rejects.toThrow(/active turn/i)

        await fake.emitSessionUpdate({
            sessionUpdate: "session_info_update",
            _meta: { piAcp: { queueDepth: 0, running: false } }
        })
        const pending = connection.setSessionConfigOption!("fake-session", "model", "fake-reasoning")
        await expect(
            connection.setSessionConfigOption!("fake-session", "model", "fake-fast")
        ).rejects.toThrow(/pending/i)

        const notificationOptions = [{
            id: "effort-after-notification",
            name: "Effort",
            category: "thought_level",
            type: "select",
            currentValue: "high",
            options: [{ value: "high", name: "High" }]
        }]
        await fake.emitSessionUpdate({
            sessionUpdate: "config_option_update",
            configOptions: notificationOptions
        })
        await fake.resolveConfigSet([{
            id: "model-from-late-response",
            name: "Model",
            category: "model",
            type: "select",
            currentValue: "fake-reasoning",
            options: [{ value: "fake-reasoning", name: "Fake Reasoning" }]
        }])

        await expect(pending).resolves.toEqual(notificationOptions)
        expect(notifications).toEqual([notificationOptions])
        expect(fake.messages.filter((message) => message.method === "session/set_config_option")).toHaveLength(1)
    })

    it("dropSession clears the old setter guard without letting its response clear a new request", async () => {
        const agentId = "agent-config-drop"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { deferConfigSet: true }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })
        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await connection.newSession("/w")
        const stale = connection.setSessionConfigOption!("fake-session", "model", "fake-reasoning")
        await vi.waitFor(() => {
            expect(fake.messages.filter((message) => message.method === "session/set_config_option"))
                .toHaveLength(1)
        })

        connection.dropSession?.("fake-session")
        await connection.newSession("/w")
        const current = connection.setSessionConfigOption!("fake-session", "effort", "medium")
        await vi.waitFor(() => {
            expect(fake.messages.filter((message) => message.method === "session/set_config_option"))
                .toHaveLength(2)
        })

        const staleResponse = [{
            id: "stale-response",
            name: "Stale",
            category: "model",
            type: "select",
            currentValue: "stale",
            options: [{ value: "stale", name: "Stale" }]
        }]
        await fake.resolveConfigSet(staleResponse)
        await expect(stale).resolves.not.toEqual(staleResponse)
        await expect(
            connection.setSessionConfigOption!("fake-session", "effort", "low")
        ).rejects.toThrow(/pending/i)

        const currentResponse = [{
            id: "current-response",
            name: "Effort",
            category: "thought_level",
            type: "select",
            currentValue: "medium",
            options: [{ value: "medium", name: "Medium" }]
        }]
        await fake.resolveConfigSet(currentResponse)
        await expect(current).resolves.toEqual(currentResponse)
    })

    it("preserves SlashCommand input/_meta from pinned adapter fixtures", async () => {
        const agentId = "agent-commands"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { commandFixture: "codex" }
        )
        const commands: unknown[] = []
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onAvailableCommands: (_sessionId, available) => commands.push(available),
            onPermissionRequest: (_sessionId, _block, choose) => choose("allow")
        })

        await connection.newSession("/w")
        await connection.prompt("fake-session", [{ type: "text", text: "commands" }])

        expect(commands).toContainEqual(PINNED_AGENT_COMMAND_FIXTURES.codex)
        expect(PINNED_AGENT_COMMAND_FIXTURES.pi[0]?.name).toBe("skill:review")
        expect(PINNED_AGENT_COMMAND_FIXTURES.claude[0]?.name).toBe("review")
        expect(PINNED_AGENT_COMMAND_FIXTURES.pi[0]?._meta).toEqual({
            yuzoraFixture: { agentId: "pi", adapterVersion: "0.0.31" }
        })
        expect(PINNED_AGENT_COMMAND_FIXTURES.claude[0]?._meta).toEqual({
            yuzoraFixture: { agentId: "claude", adapterVersion: "0.58.1" }
        })
        expect(PINNED_AGENT_COMMAND_FIXTURES.codex[0]?._meta).toEqual({
            yuzoraFixture: { agentId: "codex", adapterVersion: "1.1.2" }
        })
    })

    it("prepare initializes without creating a session", async () => {
        const agentId = "agent-prepare"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        const spawns: unknown[] = []
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") {
                spawns.push(payload)
                return agentId
            }
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ initializeTimeoutMs: 1_000 })
        await connection.prepare?.("/w")

        expect(spawns).toEqual([{ command: "bunx pi-acp@0.0.31", cwd: "/w" }])
        expect(fake.messages.filter((message) => message.method === "initialize")).toHaveLength(1)
        expect(fake.messages.find((message) => message.method === "initialize")?.params)
            .toMatchObject({ clientCapabilities: { session: { configOptions: { boolean: {} } } } })
        expect(fake.messages.some((message) => message.method === "session/new")).toBe(false)
        expect(await connection.listSessions("/w")).toEqual([])
    })

    it("records controlled cold and warm session timing with spawn removed from the warm path", async () => {
        vi.useFakeTimers()
        try {
            vi.setSystemTime(new Date("2026-07-12T00:00:00Z"))
            const agentId = "agent-controlled-timing"
            const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
            let spawnCount = 0
            mockIPC((cmd, payload) => {
                if (cmd === "agent_spawn") {
                    spawnCount += 1
                    return new Promise<string>((resolve) => {
                        setTimeout(() => resolve(agentId), 250)
                    })
                }
                if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
                if (cmd === "agent_kill") return undefined
                return undefined
            }, { shouldMockEvents: true })

            const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
            const coldStartedAt = Date.now()
            const coldSession = connection.newSession("/w")

            await vi.advanceTimersByTimeAsync(249)
            expect(spawnCount).toBe(1)
            expect(fake.messages).toEqual([])
            await vi.advanceTimersByTimeAsync(1)
            await coldSession
            const coldMs = Date.now() - coldStartedAt

            const warmStartedAt = Date.now()
            await connection.newSession("/w")
            const warmMs = Date.now() - warmStartedAt

            expect({ coldMs, warmMs }).toEqual({ coldMs: 250, warmMs: 0 })
            expect(spawnCount).toBe(1)
            expect(fake.messages.filter((message) => message.method === "initialize")).toHaveLength(1)
            expect(fake.messages.filter((message) => message.method === "session/new")).toHaveLength(2)
        } finally {
            vi.useRealTimers()
        }
    })

    it("concurrent prepare/new shares one spawn and initialize", async () => {
        const agentId = "agent-prepare-new"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        let spawnCount = 0
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") {
                spawnCount += 1
                return agentId
            }
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await Promise.all([connection.prepare?.("/w"), connection.newSession("/w")])

        expect(spawnCount).toBe(1)
        expect(fake.messages.filter((message) => message.method === "initialize")).toHaveLength(1)
        expect(fake.messages.filter((message) => message.method === "session/new")).toHaveLength(1)
    })

    it("retries initialization after prepare fails", async () => {
        let spawnCount = 0
        const second = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: "agent-2", line }))
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return `agent-${++spawnCount}`
            if (cmd === "agent_write") {
                const { id, chunk } = payload as { id: string; chunk: string }
                if (id === "agent-1") throw new Error("first initialize failed")
                return second.write(chunk)
            }
            if (cmd === "agent_stderr_tail") return []
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await expect(connection.prepare?.("/w")).rejects.toThrow("first initialize failed")
        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        expect(spawnCount).toBe(2)
    })

    it("disposes only prepared connections without owned sessions", async () => {
        let spawnCount = 0
        const kills: unknown[] = []
        const fakes = new Map<string, ReturnType<typeof createFakeAcpAgentBridge>>()
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") {
                const id = `agent-${++spawnCount}`
                fakes.set(id, createFakeAcpAgentBridge((line) => emit("agent://stdout", { id, line })))
                return id
            }
            if (cmd === "agent_write") {
                const { id, chunk } = payload as { id: string; chunk: string }
                return fakes.get(id)?.write(chunk)
            }
            if (cmd === "agent_kill") {
                kills.push(payload)
                return undefined
            }
            return undefined
        }, { shouldMockEvents: true })

        const prepared = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await prepared.prepare?.("/w")
        await expect(prepared.disposePrepared?.("/w")).resolves.toBe(true)
        expect(kills).toContainEqual({ id: "agent-1", reason: "prepared_dispose" })

        const owned = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await owned.newSession("/w")
        await expect(owned.disposePrepared?.("/w")).resolves.toBe(false)
        expect(kills).toHaveLength(1)
    })

    it("does not dispose while session/new is establishing an owner", async () => {
        const agentId = "agent-pending-owner"
        const kills: unknown[] = []
        let pendingNewId: string | number | null | undefined
        mockIPC(async (cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_kill") {
                kills.push(payload)
                return undefined
            }
            if (cmd !== "agent_write") return undefined
            const message = JSON.parse((payload as { chunk: string }).chunk.trim()) as {
                id?: string | number | null
                method?: string
            }
            if (message.method === "initialize") {
                await emit("agent://stdout", {
                    id: agentId,
                    line: JSON.stringify({
                        jsonrpc: "2.0",
                        id: message.id,
                        result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
                    })
                })
            } else if (message.method === "session/new") {
                pendingNewId = message.id
            }
            return undefined
        }, { shouldMockEvents: true })
        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })

        const pending = connection.newSession("/w")
        await vi.waitFor(() => expect(pendingNewId).toBeDefined())

        await expect(connection.disposePrepared?.("/w")).resolves.toBe(false)
        expect(kills).toEqual([])

        await emit("agent://stdout", {
            id: agentId,
            line: JSON.stringify({
                jsonrpc: "2.0",
                id: pendingNewId,
                result: { sessionId: "owned", configOptions: [] }
            })
        })
        await expect(pending).resolves.toMatchObject({ sessionId: "owned" })
    })

    it("supportsLoadSession reports true when the agent declares agentCapabilities.loadSession", async () => {
        const agentId = "agent-load-capable"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })

        await expect(connection.supportsLoadSession?.("/w")).resolves.toBe(true)
    })

    it("supportsLoadSession reports false when the agent does not declare the capability", async () => {
        const agentId = "agent-load-incapable"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { loadSessionCapability: false }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })

        await expect(connection.supportsLoadSession?.("/w")).resolves.toBe(false)
    })

    it("supportsImagePrompt 反映 initialize 的 promptCapabilities.image；未宣告視為 false", async () => {
        const capable = "agent-image-capable"
        const fakeCapable = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: capable, line }),
            { imagePromptCapability: true }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return capable
            if (cmd === "agent_write") return fakeCapable.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        // 初始化前（尚未 spawn）讀取為 false——保守 gating。
        expect(connection.supportsImagePrompt?.("s1")).toBe(false)
        await connection.newSession("/w")
        expect(connection.supportsImagePrompt?.("s1")).toBe(true)

        const incapable = "agent-image-incapable"
        const fakeIncapable = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: incapable, line })
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return incapable
            if (cmd === "agent_write") return fakeIncapable.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })
        const plain = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        await plain.newSession("/w")
        expect(plain.supportsImagePrompt?.("s1")).toBe(false)
    })

    it("image PromptBlock 以 ACP ImageContent 形狀過 wire", async () => {
        const agentId = "agent-image-wire"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { imagePromptCapability: true }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })
        const created = await connection.newSession("/w")
        await connection.prompt(created.sessionId, [
            { type: "text", text: "what is this" },
            { type: "image", data: "aGVsbG8=", mimeType: "image/png" }
        ])

        const promptMessage = fake.messages.find((m) => m.method === "session/prompt")
        expect(promptMessage?.params).toMatchObject({
            prompt: [
                { type: "text", text: "what is this" },
                { type: "image", data: "aGVsbG8=", mimeType: "image/png" }
            ]
        })
    })

    it("loadSession replays prior session/update history before resolving", async () => {
        const agentId = "agent-load-replay"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            {
                replayUpdates: [
                    { sessionUpdate: "user_message_chunk", content: { type: "text", text: "earlier question" } },
                    { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "earlier answer" } }
                ]
            }
        )
        const snapshots: TranscriptEntry[][] = []
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000,
            onTranscript: (_sessionId, transcript) => snapshots.push(transcript)
        })

        await connection.loadSession("fake-session", "/w")

        const finalTranscript = snapshots.at(-1) ?? []
        expect(finalTranscript.some((e) => "who" in e && e.who === "you" && e.text === "earlier question")).toBe(true)
        expect(finalTranscript.some((e) => "who" in e && e.who === "agent" && e.text === "earlier answer")).toBe(true)
    })

    it("loadSession resolves with the _meta.piAcp.startupInfo banner when the agent sends one", async () => {
        const agentId = "agent-load-startup-info"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { loadStartupInfo: "pi 0.0.31 · restored session" }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })

        await expect(connection.loadSession("fake-session", "/w")).resolves.toEqual({
            startupInfo: "pi 0.0.31 · restored session",
            configOptions: expect.any(Array)
        })
    })

    it("loadSession rejects when the agent responds with a JSON-RPC error", async () => {
        const agentId = "agent-load-fail"
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: agentId, line }),
            { failLoadSession: true }
        )
        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({ command: "fake-acp-agent", initializeTimeoutMs: 1_000 })

        await expect(connection.loadSession("fake-session", "/w")).rejects.toThrow()
    })

    it("exposes the spawned agent process id via processId(), undefined before connecting", async () => {
        const agentId = "agent-process-id"
        const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))

        mockIPC((cmd, payload) => {
            if (cmd === "agent_spawn") return agentId
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "fake-acp-agent",
            initializeTimeoutMs: 1_000
        })

        expect(connection.processId?.()).toBeUndefined()
        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        expect(connection.processId?.()).toBe(agentId)
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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

            await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        exited.add("agent-1")
        await emit("agent://exit", { id: "agent-1", code: 0 })

        await expect(withTestTimeout(
            connection.prompt("fake-session", [{ type: "text", text: "after exit" }]),
            100
        ))
            .rejects.toThrow("ACP agent exited")
        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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

        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
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
