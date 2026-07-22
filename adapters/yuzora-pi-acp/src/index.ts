// yuzora-pi-acp parent：對 yuzora 講 ACP v1（stdio、嚴格 LF JSONL、手寫
// JSON-RPC——不經 SDK），對內以 fork 的 session host（每 session 一個子行程，
// 完整行程隔離 extension globalThis／process.cwd）驅動 pi SDK。wire 行為以
// P1 contract fixtures（pi-acp 0.0.31 基線）為準；有意差異記錄於 spec P2。
import { fork, type ChildProcess } from "node:child_process"
import { isAbsolute } from "node:path"

import { asRecord, createLineSplitter, type HostToParent, type ParentToHost } from "./protocol.js"

const ADAPTER_VERSION = "0.1.0"
const PROTOCOL_VERSION = 1

// ---------------------------------------------------------------------------
// stdout JSON-RPC
// ---------------------------------------------------------------------------

function writeLine(message: Record<string, unknown>) {
    process.stdout.write(`${JSON.stringify(message)}\n`)
}

function respond(id: unknown, result: unknown) {
    writeLine({ jsonrpc: "2.0", id, result })
}

function respondError(id: unknown, code: number, message: string, data?: unknown) {
    writeLine({ jsonrpc: "2.0", id, error: { code, message, ...(data !== undefined ? { data } : {}) } })
}

function notify(method: string, params: Record<string, unknown>) {
    writeLine({ jsonrpc: "2.0", method, params })
}

// 對 client 的請求（session/request_permission）：字串 id 走獨立命名空間，
// 與 client 端的數字 id 永不相撞。
let nextServerRequestId = 1
const pendingClientResponses = new Map<string, (message: Record<string, unknown>) => void>()

function requestClient(method: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    const id = `srv:${nextServerRequestId++}`
    return new Promise((resolve) => {
        pendingClientResponses.set(id, resolve)
        writeLine({ jsonrpc: "2.0", id, method, params })
    })
}

// ---------------------------------------------------------------------------
// auth methods（形狀鏡射 pi-acp getAuthMethods）
// ---------------------------------------------------------------------------

const PI_SETUP_METHOD_ID = "pi_terminal_login"

function getAuthMethods(): Record<string, unknown>[] {
    return [{
        id: PI_SETUP_METHOD_ID,
        name: "Launch pi in the terminal",
        description: "Start pi in an interactive terminal to configure API keys or login",
        type: "terminal",
        args: ["--terminal-login"],
        env: {}
    }]
}

// ---------------------------------------------------------------------------
// prompt block 轉換（鏡射 pi-acp promptToPiMessage）
// ---------------------------------------------------------------------------

function promptToPiMessage(blocks: unknown): { message: string; images: { mimeType: string; data: string }[] } {
    let message = ""
    const images: { mimeType: string; data: string }[] = []
    if (!Array.isArray(blocks)) return { message, images }
    for (const raw of blocks) {
        const block = asRecord(raw)
        switch (block.type) {
            case "text":
                message += String(block.text ?? "")
                break
            case "resource_link":
                message += `\n[Context] ${String(block.uri ?? "")}`
                break
            case "image":
                if (typeof block.data === "string" && typeof block.mimeType === "string") {
                    images.push({ mimeType: block.mimeType, data: block.data })
                }
                break
            case "resource": {
                const resource = asRecord(block.resource)
                const uri = typeof resource.uri === "string" ? resource.uri : "(unknown)"
                if (typeof resource.text === "string") {
                    const mime = typeof resource.mimeType === "string" ? resource.mimeType : "text/plain"
                    message += `\n[Embedded Context] ${uri} (${mime})\n${resource.text}`
                } else {
                    message += `\n[Embedded Context] ${uri}`
                }
                break
            }
            default:
                break
        }
    }
    return { message, images }
}

// ---------------------------------------------------------------------------
// session hosts
// ---------------------------------------------------------------------------

interface ReadyInfo {
    sessionId: string
    sessionFile: string | null
    configOptions: unknown[]
    models: unknown
    modes: unknown
    startupInfo: string | null
}

interface HostHandle {
    child: ChildProcess
    nextIpcId: number
    pending: Map<number, { resolve: (data: unknown) => void; reject: (error: HostError) => void }>
    alive: boolean
}

class HostError extends Error {
    constructor(message: string, readonly authRequired = false, readonly invalidParams = false) {
        super(message)
    }
}

interface StoredSession {
    cwd: string
    sessionFile: string | null
    host?: HostHandle
}

const sessions = new Map<string, StoredSession>()

// client initialize 宣告的 elicitation.form capability（P3）；session host 據此
// 決定 select/confirm/input/editor 走 form elicitation 或降級路徑。
let clientSupportsFormElicitation = false

function hostEntryUrl(): URL {
    return new URL("./session-host.mjs", import.meta.url)
}

function spawnHost(
    sessionKey: string | null,
    init: Extract<ParentToHost, { t: "init" }>
): Promise<{ handle: HostHandle; ready: ReadyInfo }> {
    return new Promise((resolve, reject) => {
        const child = fork(hostEntryUrl(), [], {
            cwd: init.cwd,
            stdio: ["ignore", "ignore", "pipe", "ipc"],
            env: process.env
        })
        const handle: HostHandle = { child, nextIpcId: 1, pending: new Map(), alive: true }
        let settled = false
        const settleError = (error: HostError) => {
            if (settled) return
            settled = true
            reject(error)
        }
        child.stderr?.setEncoding("utf8")
        child.stderr?.on("data", (chunk: string) => process.stderr.write(`[session-host] ${chunk}`))
        child.on("error", (error) => settleError(new HostError(String(error.message ?? error))))
        child.on("exit", (code) => {
            handle.alive = false
            const error = new HostError(`pi session host exited unexpectedly (code ${code})`)
            for (const [, waiter] of handle.pending) waiter.reject(error)
            handle.pending.clear()
            settleError(error)
            const stored = sessionKey ? sessions.get(sessionKey) : undefined
            if (stored && stored.host === handle) stored.host = undefined
        })
        child.on("message", (raw) => {
            const message = raw as HostToParent
            switch (message.t) {
                case "ready":
                    if (!settled) {
                        settled = true
                        resolve({
                            handle,
                            ready: {
                                sessionId: message.sessionId,
                                sessionFile: message.sessionFile,
                                configOptions: message.configOptions,
                                models: message.models,
                                modes: message.modes,
                                startupInfo: message.startupInfo
                            }
                        })
                    }
                    break
                case "init-error":
                    settleError(new HostError(
                        message.message,
                        message.code === "auth_required",
                        message.code === "invalid_params"
                    ))
                    child.kill()
                    break
                case "update": {
                    const stored = [...sessions.entries()].find(([, candidate]) => candidate.host === handle)
                    if (stored) notify("session/update", { sessionId: stored[0], update: message.update })
                    break
                }
                case "result": {
                    const waiter = handle.pending.get(message.id)
                    if (!waiter) break
                    handle.pending.delete(message.id)
                    if (message.ok) waiter.resolve(message.data)
                    else waiter.reject(new HostError(message.message, message.authRequired, message.invalidParams))
                    break
                }
                case "dialog": {
                    const stored = [...sessions.entries()].find(([, candidate]) => candidate.host === handle)
                    if (!stored) {
                        handle.child.send({ t: "dialog-answer", reqId: message.reqId, optionId: null } satisfies ParentToHost)
                        break
                    }
                    void requestClient("session/request_permission", {
                        sessionId: stored[0],
                        toolCall: message.toolCall,
                        options: message.options
                    }).then((response) => {
                        const outcome = asRecord(asRecord(response.result).outcome)
                        const optionId = outcome.outcome === "selected" && typeof outcome.optionId === "string"
                            ? outcome.optionId
                            : null
                        if (handle.alive) {
                            handle.child.send({ t: "dialog-answer", reqId: message.reqId, optionId } satisfies ParentToHost)
                        }
                    })
                    break
                }
                case "elicit": {
                    const stored = [...sessions.entries()].find(([, candidate]) => candidate.host === handle)
                    if (!stored) {
                        handle.child.send({ t: "elicit-answer", reqId: message.reqId, action: "cancel" } satisfies ParentToHost)
                        break
                    }
                    void requestClient("elicitation/create", {
                        mode: "form",
                        sessionId: stored[0],
                        message: message.message,
                        requestedSchema: message.requestedSchema,
                        ...(message.meta ? { _meta: message.meta } : {})
                    }).then((response) => {
                        if (!handle.alive) return
                        const result = asRecord(response.result)
                        const action = result.action === "accept" || result.action === "decline"
                            ? result.action
                            : "cancel"
                        const content = asRecord(result.content)
                        handle.child.send({
                            t: "elicit-answer",
                            reqId: message.reqId,
                            action,
                            ...(action === "accept" ? { content } : {})
                        } satisfies ParentToHost)
                    })
                    break
                }
                case "log":
                    process.stderr.write(`[session-host] ${message.message}\n`)
                    break
            }
        })
        child.send(init satisfies ParentToHost)
    })
}

function hostRequest(handle: HostHandle, build: (id: number) => ParentToHost): Promise<unknown> {
    if (!handle.alive) return Promise.reject(new HostError("pi session host is not running"))
    const id = handle.nextIpcId++
    return new Promise((resolve, reject) => {
        handle.pending.set(id, { resolve, reject })
        handle.child.send(build(id))
    })
}

/** prompt／set_config 時 host 已死（或從未起）→ 以既知 sessionFile 就地復活（鏡射 pi-acp restoreSession）。 */
async function ensureHost(sessionId: string): Promise<{ stored: StoredSession; handle: HostHandle }> {
    const stored = sessions.get(sessionId)
    if (!stored) throw new HostError(`Unknown sessionId: ${sessionId}`, false, true)
    if (stored.host?.alive) return { stored, handle: stored.host }
    const { handle } = await spawnHost(sessionId, {
        t: "init",
        cwd: stored.cwd,
        formElicitation: clientSupportsFormElicitation,
        ...(stored.sessionFile ? { sessionFile: stored.sessionFile } : { loadSessionId: sessionId })
    })
    stored.host = handle
    return { stored, handle }
}

function hostErrorToRpc(id: unknown, error: unknown) {
    if (error instanceof HostError) {
        if (error.authRequired) {
            respondError(id, -32000, `Authentication required: ${error.message}`, { authMethods: getAuthMethods() })
            return
        }
        if (error.invalidParams) {
            respondError(id, -32602, error.message)
            return
        }
        respondError(id, -32603, error.message)
        return
    }
    respondError(id, -32603, error instanceof Error ? error.message : String(error))
}

// ---------------------------------------------------------------------------
// ACP method handlers
// ---------------------------------------------------------------------------

async function handleRequest(id: unknown, method: string, params: Record<string, unknown>) {
    switch (method) {
        case "initialize": {
            const requested = params.protocolVersion
            const elicitation = asRecord(asRecord(params.clientCapabilities).elicitation)
            clientSupportsFormElicitation = elicitation.form !== undefined && elicitation.form !== null
            respond(id, {
                protocolVersion: requested === PROTOCOL_VERSION ? requested : PROTOCOL_VERSION,
                agentInfo: {
                    name: "yuzora-pi-acp",
                    title: "Yuzora pi ACP adapter",
                    version: ADAPTER_VERSION
                },
                authMethods: getAuthMethods(),
                agentCapabilities: {
                    loadSession: true,
                    mcpCapabilities: { http: false, sse: false },
                    promptCapabilities: { image: true, audio: false, embeddedContext: false }
                }
            })
            return
        }
        case "authenticate":
            respond(id, null)
            return
        case "session/new": {
            const cwd = String(params.cwd ?? "")
            if (!isAbsolute(cwd)) {
                respondError(id, -32602, `cwd must be an absolute path: ${cwd}`)
                return
            }
            try {
                const { handle, ready } = await spawnHost(null, { t: "init", cwd, formElicitation: clientSupportsFormElicitation })
                sessions.set(ready.sessionId, { cwd, sessionFile: ready.sessionFile, host: handle })
                respond(id, {
                    sessionId: ready.sessionId,
                    configOptions: ready.configOptions,
                    models: ready.models,
                    modes: ready.modes,
                    _meta: { piAcp: { startupInfo: ready.startupInfo } }
                })
                if (handle.alive) handle.child.send({ t: "announce" } satisfies ParentToHost)
            } catch (error) {
                hostErrorToRpc(id, error)
            }
            return
        }
        case "session/load": {
            const cwd = String(params.cwd ?? "")
            const sessionId = String(params.sessionId ?? "")
            if (!isAbsolute(cwd)) {
                respondError(id, -32602, `cwd must be an absolute path: ${cwd}`)
                return
            }
            try {
                // reload 語意（鏡射 pi-acp）：既有 host 關掉重開，replay 才會是完整重放。
                const existing = sessions.get(sessionId)
                if (existing?.host?.alive) existing.host.child.kill()
                const { handle, ready } = await spawnHost(sessionId, {
                    t: "init",
                    cwd,
                    formElicitation: clientSupportsFormElicitation,
                    ...(existing?.sessionFile ? { sessionFile: existing.sessionFile } : { loadSessionId: sessionId })
                })
                sessions.set(sessionId, { cwd, sessionFile: ready.sessionFile, host: handle })
                await hostRequest(handle, (ipcId) => ({ t: "replay", id: ipcId }))
                respond(id, {
                    configOptions: ready.configOptions,
                    models: ready.models,
                    modes: ready.modes,
                    _meta: { piAcp: { startupInfo: null } }
                })
                if (handle.alive) handle.child.send({ t: "announce" } satisfies ParentToHost)
            } catch (error) {
                hostErrorToRpc(id, error)
            }
            return
        }
        case "session/prompt": {
            const sessionId = String(params.sessionId ?? "")
            try {
                const { handle } = await ensureHost(sessionId)
                const { message, images } = promptToPiMessage(params.prompt)
                const data = await hostRequest(handle, (ipcId) => ({ t: "prompt", id: ipcId, text: message, images }))
                respond(id, { stopReason: String(asRecord(data).stopReason ?? "end_turn") })
            } catch (error) {
                hostErrorToRpc(id, error)
            }
            return
        }
        case "session/set_config_option": {
            const sessionId = String(params.sessionId ?? "")
            const configId = String(params.configId ?? "")
            if (typeof params.value !== "string") {
                respondError(id, -32602, `Expected string value for config option: ${configId}`)
                return
            }
            const value = params.value
            try {
                const { handle } = await ensureHost(sessionId)
                const data = await hostRequest(handle, (ipcId) => ({ t: "set-config", id: ipcId, configId, value }))
                respond(id, { configOptions: asRecord(data).configOptions ?? [] })
            } catch (error) {
                hostErrorToRpc(id, error)
            }
            return
        }
        default:
            respondError(id, -32601, `Method not found: ${method}`)
    }
}

function handleNotification(method: string, params: Record<string, unknown>) {
    if (method === "session/cancel") {
        const sessionId = String(params.sessionId ?? "")
        const stored = sessions.get(sessionId)
        if (stored?.host?.alive) stored.host.child.send({ t: "cancel" } satisfies ParentToHost)
    }
}

// ---------------------------------------------------------------------------
// stdin loop
// ---------------------------------------------------------------------------

function handleLine(line: string) {
    let message: Record<string, unknown>
    try {
        message = JSON.parse(line) as Record<string, unknown>
    } catch {
        process.stderr.write(`[yuzora-pi-acp] invalid JSON line: ${line.slice(0, 160)}\n`)
        return
    }
    const hasId = message.id !== undefined && message.id !== null
    if (hasId && typeof message.method === "string") {
        void handleRequest(message.id, message.method, asRecord(message.params))
        return
    }
    if (hasId) {
        // client 對本端請求（permission）的回應。
        const key = String(message.id)
        const waiter = pendingClientResponses.get(key)
        if (waiter) {
            pendingClientResponses.delete(key)
            waiter(message)
        }
        return
    }
    if (typeof message.method === "string") handleNotification(message.method, asRecord(message.params))
}

function shutdown() {
    for (const [, stored] of sessions) {
        if (stored.host?.alive) {
            try { stored.host.child.send({ t: "shutdown" } satisfies ParentToHost) } catch { /* noop */ }
            stored.host.child.kill()
        }
    }
    process.exit(0)
}

process.stdin.setEncoding("utf8")
process.stdin.on("data", createLineSplitter(handleLine))
process.stdin.on("end", shutdown)
process.on("SIGTERM", shutdown)
