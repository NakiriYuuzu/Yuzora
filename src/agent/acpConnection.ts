import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import {
    ClientSideConnection,
    ndJsonStream,
    type AnyMessage,
    type Client,
    type Stream
} from "@zed-industries/agent-client-protocol"
import { getDocument, documentGeneration, updateBuffer } from "../editor/documentRegistry"
import { getView } from "../editor/viewRegistry"
import { saveFile } from "../lib/ipc"
import { recentlySaved } from "../lib/saveSuppress"
import type { BlockEntry, TranscriptEntry } from "./acpTypes"

const TERMINAL_OUTPUT_BYTE_LIMIT = 1_048_576

type ToolContent = Record<string, unknown>
type SessionUpdate = Record<string, unknown>

type ToolCallUpdate = {
    sessionUpdate: "tool_call" | "tool_call_update"
    toolCallId: string
    title?: string | null
    kind?: string | null
    status?: string | null
    content?: ToolContent[] | null
    locations?: { path: string; line?: number | null }[] | null
    rawInput?: Record<string, unknown>
    rawOutput?: Record<string, unknown>
}

type PlanUpdate = {
    sessionUpdate: "plan"
    entries: { content: string; priority: string; status: string }[]
}

type ToolMeta = {
    toolCallId: string
    title?: string
    kind?: string
    status?: string
    locations?: { path: string; line?: number | null }[] | null
    rawInput?: Record<string, unknown>
    rawOutput?: Record<string, unknown>
}

type PermissionKind = "allow_once" | "allow_always" | "reject_once" | "reject_always"

interface PermissionOption {
    optionId: string
    name: string
    kind: PermissionKind
}

interface RequestPermissionRequest {
    sessionId: string
    options: PermissionOption[]
    toolCall: {
        toolCallId: string
        title?: string | null
        kind?: string | null
        status?: string | null
    }
}

interface SessionNotification {
    sessionId: string
    update: SessionUpdate
}

interface RequestPermissionResponse {
    outcome:
        | { outcome: "cancelled" }
        | { outcome: "selected"; optionId: string }
}

interface WriteTextFileRequest {
    sessionId: string
    path: string
    content: string
}

interface ReadTextFileRequest {
    sessionId: string
    path: string
    line?: number | null
    limit?: number | null
}

interface ReadTextFileResponse {
    content: string
}

interface EnvVariable {
    name: string
    value: string
}

interface CreateTerminalRequest {
    sessionId: string
    command: string
    args?: string[]
    env?: EnvVariable[]
    cwd?: string | null
    outputByteLimit?: number | null
}

interface CreateTerminalResponse {
    terminalId: string
}

interface TerminalRequest {
    sessionId: string
    terminalId: string
}

interface TerminalExitStatus {
    exitCode?: number | null
    signal?: string | null
}

interface TerminalOutputResponse {
    output: string
    truncated: boolean
    exitStatus?: TerminalExitStatus | null
}

export interface SlashCommand {
    name: string
    description: string
}

export interface SessionInfoUpdate {
    queueDepth: number
    running: boolean
}

export type PromptBlock =
    | { type: "text"; text: string }
    | { type: "resource_link"; name: string; uri: string; title?: string | null }

export type StopReason = "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled"

export interface SessionMeta {
    id: string
    cwd: string
}

export const ACP_AUTH_REQUIRED_ERROR_CODE = -32000

export interface AgentAuthMethod {
    id: string
    name: string
    description?: string | null
    type?: string
    args: string[]
    env: Record<string, string>
}

export class AgentAuthRequiredError extends Error {
    readonly code = ACP_AUTH_REQUIRED_ERROR_CODE
    readonly authMethods: AgentAuthMethod[]
    readonly cwd: string | null
    readonly sessionId: string | null

    constructor({
        authMethods,
        cwd,
        sessionId = null,
        cause
    }: {
        authMethods: AgentAuthMethod[]
        cwd: string | null
        sessionId?: string | null
        cause?: unknown
    }) {
        super("Authentication required")
        this.name = "AgentAuthRequiredError"
        this.authMethods = authMethods
        this.cwd = cwd
        this.sessionId = sessionId
        if (cause !== undefined) {
            ;(this as Error & { cause?: unknown }).cause = cause
        }
    }
}

export function isAgentAuthRequiredError(value: unknown): value is AgentAuthRequiredError {
    return value instanceof AgentAuthRequiredError
        || (
            asRecord(value).name === "AgentAuthRequiredError"
            && Array.isArray(asRecord(value).authMethods)
        )
}

export interface AgentConnection {
    newSession(cwd: string): Promise<string>
    loadSession(id: string, cwd: string): Promise<void>
    listSessions(cwd: string): Promise<SessionMeta[]>
    prompt(sessionId: string, blocks: PromptBlock[]): Promise<StopReason>
    cancel(sessionId: string): void
}

interface AcpClientHandlers {
    sessionUpdate(params: SessionNotification): Promise<void>
    requestPermission(params: RequestPermissionRequest): Promise<RequestPermissionResponse>
    writeTextFile(params: WriteTextFileRequest): Promise<Record<string, never>>
    readTextFile(params: ReadTextFileRequest): Promise<ReadTextFileResponse>
    createTerminal(params: CreateTerminalRequest): Promise<CreateTerminalResponse>
    terminalOutput(params: TerminalRequest): Promise<TerminalOutputResponse>
    waitForTerminalExit(params: TerminalRequest): Promise<TerminalExitStatus>
    killTerminal(params: TerminalRequest): Promise<Record<string, never>>
    releaseTerminal(params: TerminalRequest): Promise<Record<string, never>>
}

export interface AcpClientRuntime {
    client: AcpClientHandlers
    cancelPendingPermissions(sessionId: string): void
}

export interface AcpClientRuntimeDeps {
    onTranscript?: (sessionId: string, transcript: TranscriptEntry[]) => void
    onAvailableCommands?: (sessionId: string, availableCommands: SlashCommand[]) => void
    onSessionInfo?: (sessionId: string, info: SessionInfoUpdate) => void
    onPermissionRequest?: (
        sessionId: string,
        block: BlockEntry,
        choose: (optionId: string) => void
    ) => void
}

export interface AcpConnectionDeps extends AcpClientRuntimeDeps {
    command?: string | (() => string)
    initializeTimeoutMs?: number
}

export function reduceSessionUpdate(
    transcript: TranscriptEntry[],
    update: SessionUpdate
): TranscriptEntry[] {
    const record = asRecord(update)
    const sessionUpdate = typeof record.sessionUpdate === "string" ? record.sessionUpdate : ""
    switch (sessionUpdate) {
        case "user_message_chunk":
            return appendMessage(transcript, "you", contentToText(record.content))
        case "agent_message_chunk":
            return appendMessage(transcript, "agent", contentToText(record.content))
        case "agent_thought_chunk":
        case "available_commands_update":
        case "current_mode_update":
        case "session_info_update":
            return transcript
        case "tool_call": {
            const parsed = parseToolCallUpdate(record, "tool_call")
            return parsed ? appendToolCall(transcript, parsed) : unknownSessionUpdate(transcript, sessionUpdate)
        }
        case "tool_call_update": {
            const parsed = parseToolCallUpdate(record, "tool_call_update")
            return parsed ? mergeToolCallUpdate(transcript, parsed) : unknownSessionUpdate(transcript, sessionUpdate)
        }
        case "plan":
            return upsertPlan(transcript, parsePlanUpdate(record))
        default:
            return unknownSessionUpdate(transcript, sessionUpdate)
    }
}

export function createAcpClientRuntime(deps: AcpClientRuntimeDeps = {}): AcpClientRuntime {
    const allowAlwaysToolKinds = new Set<string>()
    const pendingBySession = new Map<string, Set<() => void>>()
    const transcripts = new Map<string, TranscriptEntry[]>()

    const appendTranscript = (sessionId: string, entry: TranscriptEntry) => {
        const next = [...(transcripts.get(sessionId) ?? []), entry]
        transcripts.set(sessionId, next)
        deps.onTranscript?.(sessionId, next)
    }

    const cancelPendingPermissions = (sessionId: string) => {
        const pending = pendingBySession.get(sessionId)
        if (!pending) return
        for (const cancel of [...pending]) cancel()
    }

    return {
        client: {
            async sessionUpdate(params) {
                const update = asRecord(params.update)
                if (update.sessionUpdate === "available_commands_update") {
                    deps.onAvailableCommands?.(
                        params.sessionId,
                        normalizeAvailableCommands(update.availableCommands)
                    )
                }
                if (update.sessionUpdate === "session_info_update") {
                    const info = sessionInfoFromUpdate(update)
                    if (info) deps.onSessionInfo?.(params.sessionId, info)
                }

                const current = transcripts.get(params.sessionId) ?? []
                const next = reduceSessionUpdate(current, params.update)
                if (next !== current) {
                    transcripts.set(params.sessionId, next)
                    deps.onTranscript?.(params.sessionId, next)
                }
            },
            async writeTextFile(params) {
                const view = getView(params.path)
                recentlySaved.mark(params.path)
                if (view) {
                    view.dispatch({
                        changes: {
                            from: 0,
                            to: view.state.doc.length,
                            insert: params.content
                        }
                    })
                    updateBuffer(params.path, params.content, documentGeneration(params.path))
                }
                await saveFile(params.path, params.content)
                return {}
            },
            async readTextFile(params) {
                const view = getView(params.path)
                if (view) {
                    return { content: sliceLines(view.state.doc.toString(), params.line, params.limit) }
                }

                const entry = await getDocument(params.path)
                const result = entry.result
                if (
                    result.kind === "full"
                    || result.kind === "limited"
                    || result.kind === "nonUtf8Readonly"
                ) {
                    return { content: sliceLines(result.content, params.line, params.limit) }
                }
                throw new Error(`Cannot read non-text file: ${params.path}`)
            },
            async createTerminal(params) {
                const byteLimit = Math.min(
                    params.outputByteLimit ?? TERMINAL_OUTPUT_BYTE_LIMIT,
                    TERMINAL_OUTPUT_BYTE_LIMIT
                )
                const terminalId = await invoke<string>("agent_terminal_create", {
                    command: params.command,
                    args: params.args ?? [],
                    env: (params.env ?? []).map((item) => [item.name, item.value]),
                    cwd: params.cwd ?? ".",
                    byteLimit
                })
                return { terminalId }
            },
            async terminalOutput(params) {
                const output = normalizeTerminalOutput(
                    await invoke("agent_terminal_output", { id: params.terminalId })
                )
                if (output.output.length > 0) {
                    appendTranscript(params.sessionId, {
                        kind: "tool",
                        text: output.output,
                        meta: JSON.stringify({
                            terminalId: params.terminalId,
                            truncated: output.truncated,
                            exitStatus: output.exitStatus ?? null
                        })
                    })
                }
                return output
            },
            async waitForTerminalExit(params) {
                return normalizeExitStatus(
                    await invoke("agent_terminal_wait_for_exit", { id: params.terminalId })
                )
            },
            async killTerminal(params) {
                await invoke("agent_terminal_kill", { id: params.terminalId })
                return {}
            },
            async releaseTerminal(params) {
                await invoke("agent_terminal_release", { id: params.terminalId })
                return {}
            },
            requestPermission(params) {
                const toolKind = params.toolCall.kind ?? "other"
                if (allowAlwaysToolKinds.has(toolKind)) {
                    const rememberedOption =
                        params.options.find((option) => option.kind === "allow_always")
                        ?? params.options.find((option) => option.kind === "allow_once")
                    if (rememberedOption) {
                        return Promise.resolve(selectedPermission(rememberedOption.optionId))
                    }
                }

                return new Promise<RequestPermissionResponse>((resolve) => {
                    let settled = false
                    const finish = (response: RequestPermissionResponse) => {
                        if (settled) return
                        settled = true
                        pendingBySession.get(params.sessionId)?.delete(cancel)
                        resolve(response)
                    }
                    const cancel = () => finish({ outcome: { outcome: "cancelled" } })
                    const choose = (optionId: string) => {
                        const option = params.options.find((candidate) => candidate.optionId === optionId)
                        if (option?.kind === "allow_always") allowAlwaysToolKinds.add(toolKind)
                        finish(selectedPermission(optionId))
                    }
                    const pending = pendingBySession.get(params.sessionId) ?? new Set<() => void>()
                    pending.add(cancel)
                    pendingBySession.set(params.sessionId, pending)

                    const block = permissionBlock(params)
                    appendTranscript(params.sessionId, block)
                    deps.onPermissionRequest?.(params.sessionId, block, choose)
                    if (!deps.onPermissionRequest) cancel()
                })
            }
        },
        cancelPendingPermissions
    }
}

export function createAcpConnection(deps: AcpConnectionDeps = {}): AgentConnection {
    const runtime = createAcpClientRuntime(deps)
    const sessionRegistry = new Map<string, SessionMeta>()
    let agent: ClientSideConnection | undefined
    let agentProcessId: string | undefined
    let initialization: Promise<ClientSideConnection> | undefined
    let stdoutController: ReadableStreamDefaultController<Uint8Array> | undefined
    let disconnectedError: Error | undefined
    let authMethods: AgentAuthMethod[] = []
    const inFlightRequests = new Set<(error: Error) => void>()
    const decoder = new TextDecoder()
    const encoder = new TextEncoder()

    const rememberSession = (id: string, cwd: string) => {
        sessionRegistry.set(id, { id, cwd })
    }

    const rejectInFlightRequests = (error: Error) => {
        const pending = [...inFlightRequests]
        inFlightRequests.clear()
        for (const reject of pending) reject(error)
    }

    const trackAgentRequest = async <T>(run: () => Promise<T>): Promise<T> => {
        let rejectOnDisconnect: (error: Error) => void = () => {}
        const disconnect = new Promise<never>((_resolve, reject) => {
            rejectOnDisconnect = reject
        })
        inFlightRequests.add(rejectOnDisconnect)
        try {
            return await Promise.race([run(), disconnect])
        } finally {
            inFlightRequests.delete(rejectOnDisconnect)
        }
    }

    const markDisconnected = (error: Error) => {
        disconnectedError = error
        rejectInFlightRequests(error)
        agent = undefined
        agentProcessId = undefined
        initialization = undefined
        const controller = stdoutController
        stdoutController = undefined
        try {
            controller?.error(error)
        } catch {
            // already closed
        }
    }

    const closeStdout = () => {
        const controller = stdoutController
        stdoutController = undefined
        try {
            controller?.close()
        } catch {
            // already closed
        }
    }

    const ensureConnection = (cwd: string) => {
        if (agent) return Promise.resolve(agent)
        if (initialization) return initialization
        disconnectedError = undefined
        initialization = withTimeout(startConnection(cwd), deps.initializeTimeoutMs ?? 10_000)
            .then((connection) => {
                agent = connection
                disconnectedError = undefined
                return connection
            })
            .catch((error) => {
                initialization = undefined
                const processId = agentProcessId
                agentProcessId = undefined
                if (processId) void invoke("agent_kill", { id: processId }).catch(() => {})
                throw error
            })
        return initialization
    }

    const startConnection = async (cwd: string) => {
        const input = new ReadableStream<Uint8Array>({
            start(controller) {
                stdoutController = controller
            }
        })
        const output = new WritableStream<Uint8Array>({
            async write(chunk) {
                if (!agentProcessId) throw new Error("ACP agent process is not spawned")
                await invoke("agent_write", { id: agentProcessId, chunk: decoder.decode(chunk) })
            }
        })
        const rawStream = ndJsonStream(output, input)
        const stream = interceptUnsupportedSessionUpdates(rawStream, runtime.client, (error) => {
            markDisconnected(agentExitedError(error))
        })
        const connection = new ClientSideConnection(() => runtime.client as unknown as Client, stream)

        const unlistenStdout = await listen<{ id: string; line: string }>("agent://stdout", (event) => {
            if (event.payload.id !== agentProcessId) return
            stdoutController?.enqueue(encoder.encode(`${event.payload.line}\n`))
        })
        const unlistenExit = await listen<{ id: string; code: number | null }>("agent://exit", (event) => {
            if (event.payload.id !== agentProcessId) return
            markDisconnected(new Error("ACP agent exited"))
            unlistenStdout()
            unlistenExit()
        })

        try {
            const command = typeof deps.command === "function" ? deps.command() : deps.command
            agentProcessId = await invoke<string>("agent_spawn", {
                command: command ?? "bunx pi-acp",
                cwd
            })
            authMethods = []
            const initializeResult = await connection.initialize({
                protocolVersion: 1,
                clientCapabilities: {
                    fs: { readTextFile: true, writeTextFile: true },
                    terminal: true
                }
            })
            authMethods = normalizeAuthMethods(asRecord(initializeResult).authMethods)
            return connection
        } catch (error) {
            unlistenStdout()
            unlistenExit()
            closeStdout()
            throw error
        }
    }

    return {
        async newSession(cwd) {
            const connection = await ensureConnection(cwd)
            const result = await trackAuthRequired(
                () => trackAgentRequest(() => connection.newSession({ cwd, mcpServers: [] })),
                { cwd, sessionId: null },
                () => authMethods
            )
            rememberSession(result.sessionId, cwd)
            return result.sessionId
        },
        async loadSession(id, cwd) {
            const connection = await ensureConnection(cwd)
            await trackAgentRequest(() => connection.loadSession({ sessionId: id, cwd, mcpServers: [] }))
            rememberSession(id, cwd)
        },
        async listSessions(cwd) {
            return [...sessionRegistry.values()].filter((session) => session.cwd === cwd)
        },
        async prompt(sessionId, blocks) {
            const connection = agent
            if (!connection) throw disconnectedError ?? new Error("ACP connection is not initialized")
            const response = await trackAuthRequired(
                () => trackAgentRequest(() => connection.prompt({ sessionId, prompt: blocks })),
                { cwd: sessionRegistry.get(sessionId)?.cwd ?? null, sessionId },
                () => authMethods
            )
            return response.stopReason
        },
        cancel(sessionId) {
            runtime.cancelPendingPermissions(sessionId)
            void agent?.cancel({ sessionId }).catch(() => {})
        }
    }
}

function appendMessage(
    transcript: TranscriptEntry[],
    who: "you" | "agent",
    text: string
): TranscriptEntry[] {
    const last = transcript.at(-1)
    if (last && "who" in last && last.who === who && last.streaming) {
        return [
            ...transcript.slice(0, -1),
            { ...last, text: `${last.text}${text}`, streaming: true }
        ]
    }
    return [...transcript, { who, text, streaming: true }]
}

function permissionBlock(params: RequestPermissionRequest): BlockEntry {
    const title = params.toolCall.title ?? "Permission request"
    return {
        kind: "perm",
        text: title,
        meta: JSON.stringify({
            toolCallId: params.toolCall.toolCallId,
            kind: params.toolCall.kind ?? "other",
            status: params.toolCall.status ?? null
        }),
        actions: params.options.map((option) => ({
            label: option.name,
            kind: option.kind,
            payload: {
                optionId: option.optionId,
                toolCallId: params.toolCall.toolCallId,
                kind: option.kind
            }
        }))
    }
}

function selectedPermission(optionId: string): RequestPermissionResponse {
    return { outcome: { outcome: "selected", optionId } }
}

function normalizeAvailableCommands(value: unknown): SlashCommand[] {
    if (!Array.isArray(value)) return []
    return value.flatMap((item) => {
        const record = asRecord(item)
        return typeof record.name === "string" && typeof record.description === "string"
            ? [{ name: record.name, description: record.description }]
            : []
    })
}

function sessionInfoFromUpdate(update: unknown): SessionInfoUpdate | undefined {
    const record = asRecord(update)
    if (record.sessionUpdate !== "session_info_update") return undefined
    const meta = asRecord(record._meta)
    const piAcp = asRecord(meta.piAcp)
    return typeof piAcp.queueDepth === "number" && typeof piAcp.running === "boolean"
        ? { queueDepth: piAcp.queueDepth, running: piAcp.running }
        : undefined
}

function interceptUnsupportedSessionUpdates(
    stream: Stream,
    client: AcpClientHandlers,
    onStreamError?: (error: Error) => void
): Stream {
    return {
        writable: stream.writable,
        readable: new ReadableStream<AnyMessage>({
            async start(controller) {
                const reader = stream.readable.getReader()
                try {
                    while (true) {
                        const { value, done } = await reader.read()
                        if (done) break
                        if (isSessionInfoNotification(value)) {
                            await client.sessionUpdate(value.params as SessionNotification)
                            continue
                        }
                        controller.enqueue(value)
                    }
                    controller.close()
                } catch (error) {
                    const streamError = agentExitedError(error)
                    onStreamError?.(streamError)
                    controller.error(streamError)
                } finally {
                    reader.releaseLock()
                }
            }
        })
    }
}

function isSessionInfoNotification(value: AnyMessage): value is AnyMessage & {
    method: "session/update"
    params: SessionNotification
} {
    if (!value || typeof value !== "object" || !("method" in value)) return false
    const message = value as { method?: unknown; params?: unknown; id?: unknown }
    if (message.method !== "session/update" || "id" in message) return false
    const params = asRecord(message.params)
    const update = asRecord(params.update)
    return update.sessionUpdate === "session_info_update"
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(`ACP initialize timed out after ${ms}ms`)), ms)
    })
    return Promise.race([promise, timeoutPromise]).finally(() => {
        if (timeout) clearTimeout(timeout)
    })
}

function normalizeTerminalOutput(value: unknown): TerminalOutputResponse {
    const record = asRecord(value)
    return {
        output: typeof record.output === "string" ? record.output : "",
        truncated: record.truncated === true,
        exitStatus: record.exitStatus == null && record.exit_status == null
            ? null
            : normalizeExitStatus(record.exitStatus ?? record.exit_status)
    }
}

async function trackAuthRequired<T>(
    run: () => Promise<T>,
    context: { cwd: string | null; sessionId: string | null },
    getAuthMethods: () => AgentAuthMethod[]
): Promise<T> {
    try {
        return await run()
    } catch (error) {
        if (isAuthRequiredRpcError(error)) {
            throw new AgentAuthRequiredError({
                authMethods: getAuthMethods(),
                cwd: context.cwd,
                sessionId: context.sessionId,
                cause: error
            })
        }
        throw error
    }
}

function isAuthRequiredRpcError(error: unknown): boolean {
    const record = asRecord(error)
    const data = asRecord(record.data)
    return record.code === ACP_AUTH_REQUIRED_ERROR_CODE
        && (
            record.message === "Authentication required"
            || record.message === "auth_required"
            || data.code === "auth_required"
        )
}

function normalizeAuthMethods(value: unknown): AgentAuthMethod[] {
    if (!Array.isArray(value)) return []
    return value.flatMap((item) => {
        const record = asRecord(item)
        if (typeof record.id !== "string" || typeof record.name !== "string") return []
        return [{
            id: record.id,
            name: record.name,
            ...(typeof record.description === "string" || record.description === null
                ? { description: record.description }
                : {}),
            ...(typeof record.type === "string" ? { type: record.type } : {}),
            args: Array.isArray(record.args)
                ? record.args.filter((arg): arg is string => typeof arg === "string")
                : [],
            env: normalizeEnv(record.env)
        }]
    })
}

function normalizeEnv(value: unknown): Record<string, string> {
    const record = asRecord(value)
    const entries = Object.entries(record).filter((entry): entry is [string, string] =>
        typeof entry[1] === "string"
    )
    return Object.fromEntries(entries)
}

function normalizeExitStatus(value: unknown): TerminalExitStatus {
    const record = asRecord(value)
    return {
        exitCode: typeof record.exitCode === "number"
            ? record.exitCode
            : typeof record.exit_code === "number"
                ? record.exit_code
                : null,
        signal: typeof record.signal === "string" ? record.signal : null
    }
}

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value)
}

function firstString(...values: unknown[]): string | undefined {
    return values.find((value): value is string => typeof value === "string")
}

function agentExitedError(value: unknown): Error {
    const error = value instanceof Error
        ? value
        : new Error(typeof value === "string" ? value : "ACP agent exited")
    return error.message.includes("agent exited")
        ? error
        : new Error(`ACP agent exited: ${error.message}`)
}

function resourceToText(value: unknown): string {
    const resource = asRecord(value)
    return typeof resource.text === "string"
        ? resource.text
        : `[resource: ${typeof resource.uri === "string" ? resource.uri : "unknown"}]`
}

function sliceLines(content: string, line?: number | null, limit?: number | null): string {
    if (line == null && limit == null) return content
    const lines = content.split("\n")
    const start = Math.max(0, (line ?? 1) - 1)
    const end = limit == null ? undefined : start + Math.max(0, limit)
    return lines.slice(start, end).join("\n")
}

function unknownSessionUpdate(transcript: TranscriptEntry[], sessionUpdate: string): TranscriptEntry[] {
    return [
        ...transcript,
        {
            kind: "error",
            text: `Unknown session update: ${sessionUpdate || "unknown"}`,
            meta: JSON.stringify({ sessionUpdate: sessionUpdate || null })
        }
    ]
}

function appendToolCall(transcript: TranscriptEntry[], update: ToolCallUpdate): TranscriptEntry[] {
    const meta = toolMeta(update)
    const text = [update.title ?? "Tool call", ...toolContentText(update.content)].join("\n")
    return [
        ...transcript,
        {
            kind: "tool",
            text,
            meta: serializeToolMeta(meta)
        },
        ...diffEntries(update)
    ]
}

function mergeToolCallUpdate(
    transcript: TranscriptEntry[],
    update: ToolCallUpdate
): TranscriptEntry[] {
    const index = transcript.findIndex((entry) => {
        if (!("kind" in entry) || entry.kind !== "tool") return false
        return parseToolMeta(entry.meta)?.toolCallId === update.toolCallId
    })
    if (index === -1) {
        return appendToolCall(transcript, { ...update, sessionUpdate: "tool_call" })
    }

    const entry = transcript[index]
    if (!("kind" in entry) || entry.kind !== "tool") return transcript
    const previous = parseToolMeta(entry.meta)
    const nextMeta = {
        ...previous,
        ...toolMeta(update),
        toolCallId: update.toolCallId
    }
    const addedText = toolContentText(update.content)
    const nextEntry = {
        ...entry,
        text: addedText.length > 0 ? [entry.text, ...addedText].join("\n") : entry.text,
        meta: serializeToolMeta(nextMeta)
    }
    return [
        ...transcript.slice(0, index),
        nextEntry,
        ...transcript.slice(index + 1),
        ...diffEntries(update)
    ]
}

function upsertPlan(transcript: TranscriptEntry[], update: PlanUpdate): TranscriptEntry[] {
    const completed = update.entries.filter((entry) => entry.status === "completed").length
    const plan = {
        kind: "plan" as const,
        text: update.entries
            .map((entry) => `${planStatus(entry.status)} ${entry.content}`)
            .join("\n"),
        meta: JSON.stringify({ completed, total: update.entries.length })
    }
    const index = transcript.findIndex((entry) => "kind" in entry && entry.kind === "plan")
    if (index === -1) return [...transcript, plan]
    return [...transcript.slice(0, index), plan, ...transcript.slice(index + 1)]
}

function parseToolCallUpdate(
    record: Record<string, unknown>,
    sessionUpdate: "tool_call" | "tool_call_update"
): ToolCallUpdate | undefined {
    if (typeof record.toolCallId !== "string") return undefined
    return {
        sessionUpdate,
        toolCallId: record.toolCallId,
        title: typeof record.title === "string" ? record.title : null,
        kind: typeof record.kind === "string" ? record.kind : null,
        status: typeof record.status === "string" ? record.status : null,
        content: Array.isArray(record.content) ? record.content.map(asRecord) : null,
        locations: Array.isArray(record.locations)
            ? record.locations.flatMap((item) => {
                const location = asRecord(item)
                return typeof location.path === "string"
                    ? [{ path: location.path, line: typeof location.line === "number" ? location.line : null }]
                    : []
            })
            : null,
        rawInput: isRecord(record.rawInput) ? record.rawInput : undefined,
        rawOutput: isRecord(record.rawOutput) ? record.rawOutput : undefined
    }
}

function parsePlanUpdate(record: Record<string, unknown>): PlanUpdate {
    return {
        sessionUpdate: "plan",
        entries: Array.isArray(record.entries)
            ? record.entries.flatMap((item) => {
                const entry = asRecord(item)
                return typeof entry.content === "string"
                    && typeof entry.priority === "string"
                    && typeof entry.status === "string"
                    ? [{ content: entry.content, priority: entry.priority, status: entry.status }]
                    : []
            })
            : []
    }
}

function contentToText(content: unknown): string {
    const record = asRecord(content)
    switch (record.type) {
        case "text":
            return typeof record.text === "string" ? record.text : ""
        case "image":
            return "[image]"
        case "resource_link":
            return firstString(record.title, record.name, record.uri) ?? "[resource_link]"
        case "resource":
            return resourceToText(record.resource)
        default:
            return typeof record.type === "string" ? `[${record.type}]` : "[content]"
    }
}

function toolContentText(content: ToolContent[] | null | undefined): string[] {
    if (!content) return []
    return content.flatMap((item) => {
        const record = asRecord(item)
        if (record.type === "content") return [contentToText(record.content)]
        if (record.type === "terminal") {
            return typeof record.terminalId === "string" ? [`[terminal: ${record.terminalId}]`] : []
        }
        if (record.type === "diff") return []
        return typeof record.type === "string" ? [`[${record.type}]`] : []
    })
}

function diffEntries(update: ToolCallUpdate): TranscriptEntry[] {
    if (!update.content) return []
    return update.content.flatMap((item) => {
        const record = asRecord(item)
        if (record.type !== "diff" || typeof record.path !== "string" || typeof record.newText !== "string") {
            return []
        }
        const payload = {
            toolCallId: update.toolCallId,
            path: record.path,
            oldText: typeof record.oldText === "string" ? record.oldText : null,
            newText: record.newText
        }
        return [{
            kind: "diff" as const,
            text: record.path,
            meta: JSON.stringify(payload),
            actions: [
                { label: "View", kind: "view_diff", payload },
                { label: "Apply diff", kind: "apply_diff", payload }
            ]
        }]
    })
}

function toolMeta(update: ToolCallUpdate): ToolMeta {
    return {
        toolCallId: update.toolCallId,
        ...(update.title ? { title: update.title } : {}),
        ...(update.kind ? { kind: update.kind } : {}),
        ...(update.status ? { status: update.status } : {}),
        ...(update.locations !== undefined ? { locations: update.locations } : {}),
        ...(update.rawInput !== undefined ? { rawInput: update.rawInput } : {}),
        ...(update.rawOutput !== undefined ? { rawOutput: update.rawOutput } : {})
    }
}

function serializeToolMeta(meta: ToolMeta): string {
    return JSON.stringify(meta)
}

function parseToolMeta(meta: string | undefined): ToolMeta | undefined {
    if (!meta) return undefined
    try {
        const parsed = JSON.parse(meta) as Partial<ToolMeta>
        return typeof parsed.toolCallId === "string"
            ? { ...parsed, toolCallId: parsed.toolCallId }
            : undefined
    } catch {
        return undefined
    }
}

function planStatus(status: string): string {
    if (status === "completed") return "[x]"
    if (status === "in_progress") return "[wip]"
    return "[]"
}
