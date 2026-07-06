type JsonRpcId = string | number | null

export interface FakeAcpMessage {
    jsonrpc?: "2.0"
    id?: JsonRpcId
    method?: string
    params?: Record<string, unknown>
    result?: Record<string, unknown>
}

export interface FakeAcpAgentBridge {
    messages: FakeAcpMessage[]
    write(chunk: string): Promise<void>
}

export function createFakeAcpAgentBridge(
    emitLine: (line: string) => Promise<void>
): FakeAcpAgentBridge {
    let buffer = ""
    let permissionRequestId = 100
    let pendingPromptId: JsonRpcId | undefined
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
                    agentCapabilities: { loadSession: true },
                    authMethods: []
                }
            })
            return
        }
        if (message.method === "session/new") {
            await emit({
                jsonrpc: "2.0",
                id: message.id,
                result: { sessionId: "fake-session" }
            })
            return
        }
        if (message.method === "session/load") {
            await emit({ jsonrpc: "2.0", id: message.id, result: {} })
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
                availableCommands: [{ name: "fix", description: "Run fake fix" }]
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

    async function emit(message: FakeAcpMessage) {
        await emitLine(JSON.stringify(message))
    }

    return { messages, write }
}
