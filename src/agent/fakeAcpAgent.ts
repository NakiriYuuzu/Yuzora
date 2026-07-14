import type { AgentId } from "@/lib/agentPresets"

type JsonRpcId = string | number | null

export const PINNED_AGENT_COMMAND_FIXTURES: Record<AgentId, Record<string, unknown>[]> = {
    pi: [{
        name: "skill:review",
        description: "Review the current change",
        input: { hint: "optional focus" },
        _meta: { yuzoraFixture: { agentId: "pi", adapterVersion: "0.0.31" } }
    }],
    claude: [{
        name: "review",
        description: "Review the current change",
        input: { hint: "optional focus" },
        _meta: { yuzoraFixture: { agentId: "claude", adapterVersion: "0.58.1" } }
    }],
    codex: [{
        name: "$review",
        description: "Review the current change",
        input: null,
        _meta: { yuzoraFixture: { agentId: "codex", adapterVersion: "1.1.2" } }
    }]
}

export interface FakeAcpMessage {
    jsonrpc?: "2.0"
    id?: JsonRpcId
    method?: string
    params?: Record<string, unknown>
    result?: Record<string, unknown>
    error?: { code: number; message: string }
}

export interface FakeAcpAgentBridge {
    messages: FakeAcpMessage[]
    write(chunk: string): Promise<void>
    emitSessionUpdate(update: Record<string, unknown>): Promise<void>
    resolveConfigSet(configOptions?: Record<string, unknown>[]): Promise<void>
    rejectConfigSet(message?: string): Promise<void>
}

export interface FakeAcpAgentOptions {
    // Declared agentCapabilities.loadSession in the initialize response.
    // Defaults to true (existing behavior/tests rely on this).
    loadSessionCapability?: boolean
    // Declared agentCapabilities.promptCapabilities.image in the initialize
    // response. Defaults to false — image support is feature-detected.
    imagePromptCapability?: boolean
    // session/update payloads to replay (in order) before responding to
    // session/load — simulates an agent restoring prior transcript history.
    replayUpdates?: Record<string, unknown>[]
    // When true, session/load responds with a JSON-RPC error instead of
    // replaying/succeeding — simulates the agent losing/rejecting the session.
    failLoadSession?: boolean
    // When set, session/load's result carries _meta.piAcp.startupInfo (mirrors
    // session/new's startupInfo banner) — feature-detected by the client.
    loadStartupInfo?: string
    // Emit adapter-shaped available command metadata for pin-sensitive tests.
    commandFixture?: AgentId
    // Hold session/set_config_option responses until resolveConfigSet/rejectConfigSet.
    deferConfigSet?: boolean
}

export function createFakeAcpAgentBridge(
    emitLine: (line: string) => Promise<void>,
    options: FakeAcpAgentOptions = {}
): FakeAcpAgentBridge {
    let buffer = ""
    const permissionRequestId = 100
    let pendingPromptId: JsonRpcId | undefined
    let model = "fake-fast"
    let effort = "low"
    let autoExecute = true
    const pendingConfigSets: JsonRpcId[] = []
    const messages: FakeAcpMessage[] = []

    async function write(chunk: string) {
        buffer += chunk
        const lines = buffer.split("\n")
        buffer = lines.pop() ?? ""
        for (const line of lines) {
            const trimmed = line.trim()
            if (trimmed.length === 0) continue
            await handleMessage(JSON.parse(trimmed) as FakeAcpMessage)
        }
    }

    async function handleMessage(message: FakeAcpMessage) {
        messages.push(message)
        if (message.method === "initialize") {
            await emit({
                jsonrpc: "2.0",
                id: message.id,
                result: {
                    protocolVersion: 1,
                    agentCapabilities: {
                        loadSession: options.loadSessionCapability ?? true,
                        ...(options.imagePromptCapability !== undefined
                            ? { promptCapabilities: { image: options.imagePromptCapability } }
                            : {})
                    },
                    authMethods: []
                }
            })
            return
        }
        if (message.method === "session/new") {
            await emit({
                jsonrpc: "2.0",
                id: message.id,
                result: {
                    sessionId: "fake-session",
                    configOptions: currentConfigOptions(),
                    _meta: { piAcp: { startupInfo: "pi 0.0.31 · ## Context(AGENTS.md)" } },
                }
            })
            return
        }
        if (message.method === "session/load") {
            if (options.failLoadSession) {
                await emit({
                    jsonrpc: "2.0",
                    id: message.id,
                    error: { code: -32001, message: "Session not found" }
                })
                return
            }
            for (const update of options.replayUpdates ?? []) {
                await emitSessionUpdate(update)
            }
            await emit({
                jsonrpc: "2.0",
                id: message.id,
                result: options.loadStartupInfo
                    ? {
                        configOptions: currentConfigOptions(),
                        _meta: { piAcp: { startupInfo: options.loadStartupInfo } }
                    }
                    : { configOptions: currentConfigOptions() }
            })
            return
        }
        if (message.method === "session/set_config_option") {
            const params = message.params ?? {}
            if (params.configId === "model" && typeof params.value === "string") {
                model = params.value
                effort = model === "fake-reasoning" ? "high" : "low"
            } else if (params.configId === "effort" && typeof params.value === "string") {
                effort = params.value
            } else if (params.configId === "auto-execute" && typeof params.value === "boolean") {
                autoExecute = params.value
            }
            if (options.deferConfigSet) {
                pendingConfigSets.push(message.id ?? null)
            } else {
                await emit({
                    jsonrpc: "2.0",
                    id: message.id,
                    result: { configOptions: currentConfigOptions() }
                })
            }
            return
        }
        if (message.method === "session/prompt") {
            pendingPromptId = message.id
            await emitSessionUpdate({
                sessionUpdate: "session_info_update",
                _meta: { piAcp: { queueDepth: 0, running: true } }
            })
            await emitSessionUpdate({
                sessionUpdate: "available_commands_update",
                availableCommands: options.commandFixture
                    ? PINNED_AGENT_COMMAND_FIXTURES[options.commandFixture]
                    : [{ name: "fix", description: "Run fake fix" }]
            })
            await emitSessionUpdate({
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "Ready" }
            })
            await emit({
                jsonrpc: "2.0",
                id: permissionRequestId,
                method: "session/request_permission",
                params: {
                    sessionId: "fake-session",
                    toolCall: {
                        toolCallId: "fake|permission",
                        title: "Edit file",
                        kind: "edit",
                        status: "pending"
                    },
                    options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }]
                }
            })
            return
        }
        if (message.id === permissionRequestId && message.result) {
            await emitSessionUpdate({
                sessionUpdate: "tool_call",
                toolCallId: "call_fake|edit_1",
                title: "write",
                kind: "edit",
                status: "pending",
                rawInput: {}
            })
            await emitSessionUpdate({
                sessionUpdate: "tool_call_update",
                toolCallId: "call_fake|edit_1",
                status: "completed",
                content: [{ type: "diff", path: "hello.txt", oldText: null, newText: "hi\n" }]
            })
            await emitSessionUpdate({
                sessionUpdate: "session_info_update",
                _meta: { piAcp: { queueDepth: 0, running: false } }
            })
            await emit({
                jsonrpc: "2.0",
                id: pendingPromptId,
                result: { stopReason: "end_turn" }
            })
        }
    }

    async function emitSessionUpdate(update: Record<string, unknown>) {
        await emit({
            jsonrpc: "2.0",
            method: "session/update",
            params: { sessionId: "fake-session", update }
        })
    }

    async function resolveConfigSet(
        configOptions: Record<string, unknown>[] = currentConfigOptions()
    ): Promise<void> {
        const id = pendingConfigSets.shift()
        if (id === undefined) throw new Error("No deferred config setter is pending")
        await emit({ jsonrpc: "2.0", id, result: { configOptions } })
    }

    async function rejectConfigSet(message = "Config setter failed"): Promise<void> {
        const id = pendingConfigSets.shift()
        if (id === undefined) throw new Error("No deferred config setter is pending")
        await emit({ jsonrpc: "2.0", id, error: { code: -32002, message } })
    }

    async function emit(message: FakeAcpMessage) {
        await emitLine(JSON.stringify(message))
    }

    function currentConfigOptions(): Record<string, unknown>[] {
        const effortOptions = model === "fake-reasoning"
            ? [
                { value: "medium", name: "Medium" },
                { value: "high", name: "High" }
            ]
            : [
                { value: "low", name: "Low" },
                { value: "medium", name: "Medium" }
            ]
        return [
            {
                id: "model",
                name: "Model",
                category: "model",
                type: "select",
                currentValue: model,
                options: [
                    {
                        group: "standard",
                        name: "Standard",
                        options: [{ value: "fake-fast", name: "Fake Fast" }]
                    },
                    {
                        group: "reasoning",
                        name: "Reasoning",
                        options: [{ value: "fake-reasoning", name: "Fake Reasoning" }]
                    }
                ]
            },
            {
                id: "effort",
                name: "Effort",
                category: "thought_level",
                type: "select",
                currentValue: effort,
                options: effortOptions
            },
            {
                id: "auto-execute",
                name: "Auto execute",
                category: "_fake",
                type: "boolean",
                currentValue: autoExecute
            }
        ]
    }

    return { messages, write, emitSessionUpdate, resolveConfigSet, rejectConfigSet }
}
