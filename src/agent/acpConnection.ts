import { listen } from "@tauri-apps/api/event"
import type { AgentCommandIdentity, AgentId } from "@/lib/agentPresets"
import { DEFAULT_AGENT_COMMAND } from "@/lib/agentPresets"
import { invoke } from "@/lib/ipc"
import {
    ClientSideConnection,
    ndJsonStream,
    type AnyMessage,
    type Client,
    type Stream
} from "@agentclientprotocol/sdk"
import { resolveAgentSpawnCommand } from "./agentRuntime"
import { getDocument, documentGeneration, updateBuffer } from "../editor/documentRegistry"
import { normalizeDocumentLineEndings, serializeDocumentLineEndings } from "../editor/lineEndings"
import { getView } from "../editor/viewRegistry"
import { saveFile } from "../lib/ipc"
import { recentlySaved } from "../lib/saveSuppress"
import { useWorkspaceStore } from "../state/workspaceStore"
import { newEntryId, type BlockEntry, type TranscriptEntry } from "./acpTypes"

const TERMINAL_OUTPUT_BYTE_LIMIT = 1_048_576

// default case（見 reduceSessionUpdate）用來記錄「已 warn 過」的變體名，同一變體只 warn 一次，
// 避免高頻更新（如未來 SDK 新增的變體）洗版 console。
const warnedUnknownUpdates = new Set<string>()

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
    /** claude-agent-acp：子 agent 內部 tool call 歸屬（_meta.claudeCode.parentToolUseId）。 */
    parentToolCallId?: string
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
    /** sub-agent 內部 tool call → spawn 它的 tool call（UI 據此嵌套呈現）。 */
    parentToolCallId?: string
    // diff content 的行數統計（2026-07-21 使用者回饋：composer 上方顯示變更彙總）。
    // 連線層在收到 diff record 當下計算，UI 端只彙總、不再持有 oldText/newText。
    diffs?: { path: string; added: number; removed: number }[]
}

// ACP elicitation（UNSTABLE capability；P3／P4）：form mode 的請求化約為 UI 可
// 直接渲染的欄位清單。支援 primitive 欄位（string／boolean／number／integer、
// string enum/oneOf 單選）與 array multiselect（items.enum／items.anyOf，P4
// question 多選）；其餘型別回 null → cancel。
export interface ElicitationField {
    key: string
    type: "string" | "boolean" | "number" | "integer" | "array"
    title?: string
    description?: string
    required: boolean
    /** request _meta.yuzora.multiline（editor 語意）→ textarea 呈現。 */
    multiline?: boolean
    defaultValue?: string | number | boolean | string[]
    /** string enum／oneOf → 單選選項；array → multiselect 選項。 */
    options?: { value: string; label: string; description?: string }[]
}

export interface ElicitationRequest {
    message: string
    title?: string
    fields: ElicitationField[]
}

export type ElicitationResponsePayload =
    | { action: "accept"; content: Record<string, string | number | boolean | string[]> }
    | { action: "decline" }
    | { action: "cancel" }

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
    input?: Record<string, unknown> | null
    _meta?: Record<string, unknown> | null
}

export interface SessionConfigSelectOption {
    value: string
    name: string
    description?: string | null
    _meta?: Record<string, unknown> | null
}

export interface SessionConfigSelectGroup {
    group: string
    name: string
    options: SessionConfigSelectOption[]
    _meta?: Record<string, unknown> | null
}

interface SessionConfigOptionBase {
    id: string
    name: string
    description?: string | null
    category?: string | null
    _meta?: Record<string, unknown> | null
}

export type SessionConfigOption = SessionConfigOptionBase & (
    | {
        type: "select"
        currentValue: string
        options: SessionConfigSelectOption[] | SessionConfigSelectGroup[]
    }
    | { type: "boolean"; currentValue: boolean }
)

export type SessionConfigValue = string | boolean

export interface SessionInfoUpdate {
    queueDepth: number
    running: boolean
}

export interface UsageInfo {
    used: number
    size: number
    cost?: { amount: number; currency: string }
}

export type PromptBlock =
    | { type: "text"; text: string }
    | { type: "resource_link"; name: string; uri: string; title?: string | null }
    // 對齊 ACP SDK ImageContent 的必填欄位：data 為「純 base64」（無 data: 前綴）。
    | { type: "image"; data: string; mimeType: string }

export type StopReason = "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled"

export interface SessionMeta {
    id: string
    cwd: string
}

const ACP_AUTH_REQUIRED_ERROR_CODE = -32000
const ACP_INVALID_PARAMS_ERROR_CODE = -32602
const AGENT_KILL_TIMEOUT_MS = 5_000
// #37：bunx／npx 首次執行時會先從 registry 下載 adapter，這段時間內 adapter
// 還沒開始講 ACP。給它獨立的冷啟動視窗，別讓下載時間吃掉 protocol handshake
// 的短 timeout。
const BOOTSTRAP_TIMEOUT_MS = 120_000
const INITIALIZE_TIMEOUT_MS = 15_000

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
    readonly agentCommand?: string
    readonly agentIdentity?: AgentCommandIdentity

    constructor({
        authMethods,
        cwd,
        sessionId = null,
        agentCommand,
        agentIdentity,
        cause
    }: {
        authMethods: AgentAuthMethod[]
        cwd: string | null
        sessionId?: string | null
        agentCommand?: string
        agentIdentity?: AgentCommandIdentity
        cause?: unknown
    }) {
        super("Authentication required")
        this.name = "AgentAuthRequiredError"
        this.authMethods = authMethods
        this.cwd = cwd
        this.sessionId = sessionId
        this.agentCommand = agentCommand
        this.agentIdentity = agentIdentity
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

export type SessionNewFailureKind = "auth-required" | "transport" | "protocol"

export function classifySessionNewFailure(
    error: unknown,
    disconnected = false
): SessionNewFailureKind {
    if (isAgentAuthRequiredError(error)) return "auth-required"
    return disconnected ? "transport" : "protocol"
}

interface NewSessionResult {
    sessionId: string
    startupInfo: string | null
    agentVersion?: string
    /** initialize.agentInfo.name（如 "pi-acp"／"yuzora-pi-acp"）——P5 runtime badge 的判斷來源。 */
    agentName?: string
    configOptions?: SessionConfigOption[]
    agentIdentity?: AgentCommandIdentity
    /** SHA-256 identity for a custom command. The raw command is never persisted. */
    customCommandFingerprint?: string
}

interface LoadSessionResult {
    startupInfo: string | null
    agentVersion?: string
    agentName?: string
    configOptions?: SessionConfigOption[]
    agentIdentity?: AgentCommandIdentity
}

export interface AgentConnection {
    prepare?(cwd: string, agentId?: AgentId): Promise<void>
    newSession(cwd: string, agentId?: AgentId): Promise<NewSessionResult>
    // Union with `void` so existing stubs/mocks that resolve to nothing (most
    // tests don't care about replayed startupInfo) stay valid without change.
    // agentId（optional）：restored session 續聊時的路由提示，供 agentRouter 決定
    // unknown sessionId 該走哪個 sub connection；單連線實作（本檔）忽略之，照現行為。
    loadSession(
        id: string,
        cwd: string,
        agentId?: AgentId,
        customCommandFingerprint?: string
    ): Promise<LoadSessionResult | void>
    listSessions(cwd: string): Promise<SessionMeta[]>
    prompt(sessionId: string, blocks: PromptBlock[]): Promise<StopReason>
    cancel(sessionId: string): void | Promise<void>
    setSessionConfigOption?(
        sessionId: string,
        configId: string,
        value: SessionConfigValue
    ): Promise<SessionConfigOption[]>
    disposePrepared?(cwd?: string): Promise<boolean>
    disposeOwnerless?(reason: string): Promise<boolean>
    pendingTeardown?(): Promise<void> | undefined
    retryCooldownUntil?(cwd: string, agentId?: AgentId): number
    processId?(): string | undefined
    // Ensures a connection for cwd (spawning if needed) and reports whether the
    // agent declared the loadSession capability at initialize time. Used by the
    // Continue-session flow to decide between replay and the degrade path.
    supportsLoadSession?(
        cwd: string,
        agentId?: AgentId,
        customCommandFingerprint?: string
    ): Promise<boolean>
    // Synchronous feature-detection read (C3): whether the session's agent
    // declared promptCapabilities.image at initialize time. Unknown session or
    // pre-initialize state reads as false — the composer hides image entry
    // points rather than guessing.
    supportsImagePrompt?(sessionId: string): boolean
    // F10：removeSession 收尾用——通知連線把該 session 的 runtime 內部狀態
    // （transcript／pending permission）清掉。找不到對應 sub／session 時靜默略過。
    dropSession?(sessionId: string): void
}

interface AcpClientHandlers {
    sessionUpdate(params: SessionNotification): Promise<void>
    requestPermission(params: RequestPermissionRequest): Promise<RequestPermissionResponse>
    // ACP UNSTABLE：elicitation/create（form mode）。SDK Client interface 的
    // optional method——實作即被 route；未知 mode 一律回 cancel（graceful degradation）。
    unstable_createElicitation(params: unknown): Promise<Record<string, unknown>>
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
    recordUserPrompt(sessionId: string, blocks: PromptBlock[]): void
    // F10：removeSession 收尾用——把該 session 的 transcript／pending permission
    // 記錄從 runtime 內部的 Map 中清掉，避免長駐 in-memory 累積。
    dropSession(sessionId: string): void
}

export interface AcpClientRuntimeDeps {
    onTranscript?: (sessionId: string, transcript: TranscriptEntry[]) => void
    onAvailableCommands?: (sessionId: string, availableCommands: SlashCommand[]) => void
    onSessionInfo?: (sessionId: string, info: SessionInfoUpdate) => void
    onUsage?: (sessionId: string, usage: UsageInfo) => void
    onSessionTitle?: (sessionId: string, title: string) => void
    onConfigOptions?: (sessionId: string, configOptions: SessionConfigOption[]) => void
    onPermissionRequest?: (
        sessionId: string,
        block: BlockEntry,
        choose: (optionId: string) => void
    ) => void
    onElicitationRequest?: (
        sessionId: string,
        request: ElicitationRequest,
        respond: (response: ElicitationResponsePayload) => void
    ) => void
}

export interface AcpConnectionDeps extends AcpClientRuntimeDeps {
    command?: string | (() => string)
    /** 第一個 stdout byte → initialize 回應的上限（ACP protocol handshake）。 */
    initializeTimeoutMs?: number
    /** agent_spawn → 第一個 stdout byte 的上限（#37 冷啟動下載視窗）。 */
    bootstrapTimeoutMs?: number
}

/** 進行中的冷啟動（bunx／npx 首次下載 adapter）；UI 據此顯示進度與取消。 */
export interface AgentColdStart {
    processId: string
    /** 白名單化的 runtime 名（見 resolveCommandRuntime）；未知一律 "other"。 */
    runtime: string
    /** 這條連線的 workspace cwd。取消是 per-route 的，UI 據此分辨是哪一條。 */
    cwd: string
    startedAt: number
    /** 已按下取消、agent_kill 仍在飛：按鈕 disabled，記錄不得先被移除。 */
    cancelling: boolean
}

interface ColdStartRecord {
    entry: AgentColdStart
    kill: () => Promise<void>
    cancelling?: Promise<void>
    released: boolean
}

const coldStarts = new Map<string, ColdStartRecord>()
const coldStartListeners = new Set<() => void>()
let coldStartSnapshot: AgentColdStart[] = []

function publishColdStarts(): void {
    coldStartSnapshot = [...coldStarts.values()].map((item) => item.entry)
    for (const listener of [...coldStartListeners]) listener()
}

export function subscribeAgentColdStarts(listener: () => void): () => void {
    coldStartListeners.add(listener)
    return () => {
        coldStartListeners.delete(listener)
    }
}

/** useSyncExternalStore 相容：同一批狀態回傳同一個陣列實體。 */
export function agentColdStarts(): AgentColdStart[] {
    return coldStartSnapshot
}

/**
 * 使用者取消單一條冷啟動下載（per-route：只收掉這個 processId，不動其他同時在
 * bootstrap 的連線）。kill 必須 await（Rust 的 kill_tree 已含 bounded wait），
 * 且這筆記錄要留到 kill 回報後才移除，否則畫面已回復時 npm／node／cmd 的子孫
 * 行程可能還活著。
 */
export async function cancelAgentColdStart(processId: string): Promise<void> {
    const record = coldStarts.get(processId)
    if (!record) return
    if (record.cancelling) return record.cancelling
    const cancelling = record.kill().finally(() => {
        record.cancelling = undefined
        if (record.released) coldStarts.delete(processId)
        publishColdStarts()
    })
    record.cancelling = cancelling
    record.entry = { ...record.entry, cancelling: true }
    publishColdStarts()
    await cancelling
}

function registerColdStart(entry: AgentColdStart, kill: () => Promise<void>): void {
    coldStarts.set(entry.processId, { entry, kill, released: false })
    publishColdStarts()
}

function releaseColdStart(processId: string): void {
    const record = coldStarts.get(processId)
    if (!record) return
    // 取消中：kill 尚未回報前不得讓 banner 消失（子孫行程可能還在收）
    if (record.cancelling) {
        record.released = true
        return
    }
    coldStarts.delete(processId)
    publishColdStarts()
}

// #37：spawn 指令的 runtime 判定與 telemetry 都以 resolveCommandRuntime 為準。
// 指令可能帶 `KEY=value` env 前綴（本專案的 pi 憑證正是 env-var 制）、含空白的
// 引號路徑、`cmd /c` 之類的 wrapper，或 `bun x`／`pnpm dlx` 這種雙 token 形式；
// 弱 tokenizer 會同時造成「secret 寫進 log」與「真的會下載的指令拿不到冷啟動
// 視窗」。對齊 Rust 端 agent_process.rs 的 command_first_token（跳過 env 前綴、
// 完整 command 不入 log）。
const REGISTRY_RUNTIMES = new Set(["bunx", "npx"])
/** 雙 token 形式：`bun x`／`pnpm dlx`／`yarn dlx` 一樣會從 registry 取套件。 */
const REGISTRY_SUBCOMMANDS: Record<string, string> = { bun: "x", pnpm: "dlx", yarn: "dlx" }
/** wrapper：真正的 runtime 在 `/c`／`-c` 之後（Windows 與 shell 包裝）。 */
const WRAPPER_RUNTIMES = new Set(["cmd", "sh", "bash", "zsh", "pwsh", "powershell"])
const WRAPPER_FLAGS = new Set(["/c", "-c", "-command"])
/** telemetry 白名單。不在此列的 token 一律回報 "other"，絕不原樣寫進 log。 */
const KNOWN_RUNTIMES = new Set([
    "bunx", "npx", "bun", "npm", "pnpm", "yarn", "node", "deno",
    "cmd", "sh", "bash", "zsh", "pwsh", "powershell"
])
const ENV_ASSIGNMENT = /^[A-Za-z_][A-Za-z0-9_]*=/

interface CommandRuntime {
    /** 白名單化的 runtime 名；未知一律 "other"。 */
    runtime: string
    /** 會從 package registry 下載，需要冷啟動視窗。 */
    registry: boolean
}

const UNKNOWN_RUNTIME: CommandRuntime = { runtime: "other", registry: false }

/** 感知引號的切詞：`"C:\Program Files\nodejs\npx.cmd"` 必須是單一 token。 */
function tokenizeCommand(command: string): string[] {
    const tokens: string[] = []
    let current = ""
    let quote: string | undefined
    let open = false
    for (const char of command) {
        if (quote) {
            if (char === quote) quote = undefined
            else current += char
            continue
        }
        if (char === '"' || char === "'") {
            quote = char
            open = true
            continue
        }
        if (/\s/.test(char)) {
            if (open) tokens.push(current)
            current = ""
            open = false
            continue
        }
        current += char
        open = true
    }
    if (open) tokens.push(current)
    return tokens
}

function binaryName(token: string): string {
    const base = token.split(/[\\/]/).pop() ?? ""
    return base.toLowerCase().replace(/\.(cmd|exe|bat|ps1)$/, "")
}

function resolveCommandRuntime(command: string): CommandRuntime {
    let tokens = tokenizeCommand(command)
    // wrapper 最多拆兩層（`cmd /c sh -c npx …`）；再深就不猜了
    for (let depth = 0; depth < 3; depth += 1) {
        const start = tokens.findIndex((token) => !ENV_ASSIGNMENT.test(token))
        if (start < 0) return UNKNOWN_RUNTIME
        tokens = tokens.slice(start)
        const name = binaryName(tokens[0] ?? "")
        if (WRAPPER_RUNTIMES.has(name)) {
            const flag = tokens.findIndex(
                (token, index) => index > 0 && WRAPPER_FLAGS.has(token.toLowerCase())
            )
            if (flag < 0) return { runtime: name, registry: false }
            const inner = tokens.slice(flag + 1)
            // `sh -c "npx -y …"`：整串指令會被引號收成單一 token
            tokens = inner.length === 1 ? tokenizeCommand(inner[0] ?? "") : inner
            continue
        }
        const runtime = KNOWN_RUNTIMES.has(name) ? name : "other"
        if (REGISTRY_RUNTIMES.has(name)) return { runtime, registry: true }
        const subcommand = REGISTRY_SUBCOMMANDS[name]
        if (subcommand !== undefined && subcommand === tokens[1]?.toLowerCase()) {
            return { runtime, registry: true }
        }
        // `deno run -A npm:pi-acp@0.0.32`：npm: specifier 才會下載
        if (name === "deno" && tokens.some((token) => token.startsWith("npm:"))) {
            return { runtime, registry: true }
        }
        return { runtime, registry: false }
    }
    return UNKNOWN_RUNTIME
}

function usesRegistryRuntime(command: string): boolean {
    return resolveCommandRuntime(command).registry
}

type StartupPhase = "connected" | "cold_start_timeout" | "init_timeout" | "bootstrap_cancelled" | "failed"

interface StartupTiming {
    bootstrapMs?: number
    initializeMs?: number
}

// #37 duration telemetry。只送白名單化的 runtime 名與兩段耗時——完整 command 可能
// 帶 env 前綴的憑證、registry 認證或私有 package 路徑，一律不入 log（token 同理）。
function logStartupTiming(phase: StartupPhase, command: string, timing: StartupTiming): void {
    void invoke("log_event", {
        event: {
            level: phase === "connected" ? "info" : "warn",
            kind: "debug",
            source: "acp",
            workspace_path: null,
            event: `agent_startup_${phase}`,
            message: `ACP agent startup: ${phase}`,
            metadata: { runtime: resolveCommandRuntime(command).runtime, ...timing }
        }
    }).catch(() => undefined)
}

export function reduceSessionUpdate(
    transcript: TranscriptEntry[],
    update: SessionUpdate
): TranscriptEntry[] {
    const record = asRecord(update)
    const sessionUpdate = typeof record.sessionUpdate === "string" ? record.sessionUpdate : ""
    switch (sessionUpdate) {
        case "user_message_chunk": {
            // Replayed (session/load) user turns may carry image content blocks.
            // Attach them to the streaming user entry as thumbnails instead of
            // collapsing to the "[image]" text placeholder (plan t4-3c). A block
            // missing data/mimeType falls through to the text path silently.
            const content = asRecord(record.content)
            if (
                content.type === "image" &&
                typeof content.data === "string" &&
                typeof content.mimeType === "string"
            ) {
                return appendUserImage(transcript, {
                    mimeType: content.mimeType,
                    dataUrl: `data:${content.mimeType};base64,${content.data}`
                })
            }
            return appendMessage(transcript, "you", contentToText(record.content))
        }
        case "agent_message_chunk":
            return appendMessage(transcript, "agent", contentToText(record.content))
        case "agent_thought_chunk":
            return appendThought(transcript, contentToText(record.content))
        case "available_commands_update":
        case "current_mode_update":
        case "session_info_update":
        case "usage_update":
        case "config_option_update":
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
            // 這裡攔到的是「SDK schema 認得、本端 switch 尚未處理」的變體（如 plan_update／
            // plan_removed，或未來 SDK 升級新增者）——官方 SDK 的 zod 驗證已在更上游擋掉
            // SDK 層也未知的變體與 malformed tool_call，不會落到這裡。靜默略過、不渲染成
            // error block，同一變體僅 warn 一次供排查（見 warnedUnknownUpdates）。
            if (!warnedUnknownUpdates.has(sessionUpdate)) {
                warnedUnknownUpdates.add(sessionUpdate)
                console.warn(`Unknown session update: ${sessionUpdate || "unknown"}`)
            }
            return transcript
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

    const recordUserPrompt = (sessionId: string, blocks: PromptBlock[]) => {
        const text = promptBlocksToText(blocks)
        if (!text) return
        // 先把先前 entries 的 streaming 收斂，否則下一輪 onTranscript 覆蓋會讓上一輪
        // 已結束的訊息又變回 streaming:true（殘留游標）。
        const settled = (transcripts.get(sessionId) ?? []).map((entry) =>
            "streaming" in entry ? { ...entry, streaming: false } : entry
        )
        const next = [
            ...settled,
            { id: newEntryId(), who: "you" as const, text, streaming: true },
        ]
        transcripts.set(sessionId, next)
        // 刻意不呼叫 deps.onTranscript：beginTurn 已在 store 樂觀顯示。
    }

    const cancelPendingPermissions = (sessionId: string) => {
        const pending = pendingBySession.get(sessionId)
        if (!pending) return
        for (const cancel of [...pending]) cancel()
    }

    const dropSession = (sessionId: string) => {
        transcripts.delete(sessionId)
        pendingBySession.delete(sessionId)
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
                    const title = sessionTitleFromUpdate(update)
                    if (title) deps.onSessionTitle?.(params.sessionId, title)
                }
                if (update.sessionUpdate === "usage_update") {
                    const usage = usageFromUpdate(update)
                    if (usage) deps.onUsage?.(params.sessionId, usage)
                }
                if (update.sessionUpdate === "config_option_update") {
                    deps.onConfigOptions?.(
                        params.sessionId,
                        normalizeSessionConfigOptions(update.configOptions)
                    )
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
                const normalizedContent = normalizeDocumentLineEndings(params.content)
                let writableContent = params.content
                if (view) {
                    const lineEnding = useWorkspaceStore.getState().getLineEnding(params.path)
                    if (!lineEnding) {
                        throw new Error("Line-ending metadata is unavailable for the open document")
                    }
                    const serialized = serializeDocumentLineEndings(normalizedContent, lineEnding)
                    if (serialized.kind === "blocked") {
                        throw new Error("Mixed line endings require an explicit LF or CRLF selection")
                    }
                    writableContent = serialized.content
                    view.dispatch({
                        changes: {
                            from: 0,
                            to: view.state.doc.length,
                            insert: normalizedContent
                        }
                    })
                    updateBuffer(params.path, normalizedContent, documentGeneration(params.path))
                }
                recentlySaved.mark(params.path)
                await saveFile(params.path, writableContent)
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
                        id: newEntryId(),
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
            },
            unstable_createElicitation(params) {
                const record = asRecord(params)
                const sessionId = typeof record.sessionId === "string" ? record.sessionId : null
                const request = record.mode === "form" ? normalizeElicitationRequest(record) : null
                if (!sessionId || !request) {
                    return Promise.resolve({ action: "cancel" })
                }
                return new Promise<Record<string, unknown>>((resolve) => {
                    let settled = false
                    const finish = (response: Record<string, unknown>) => {
                        if (settled) return
                        settled = true
                        pendingBySession.get(sessionId)?.delete(cancel)
                        resolve(response)
                    }
                    // session/cancel／dropSession 與 permission 共用同一個 cancel set。
                    const cancel = () => finish({ action: "cancel" })
                    const respond = (response: ElicitationResponsePayload) => {
                        finish(response.action === "accept"
                            ? { action: "accept", content: response.content }
                            : { action: response.action })
                    }
                    const pending = pendingBySession.get(sessionId) ?? new Set<() => void>()
                    pending.add(cancel)
                    pendingBySession.set(sessionId, pending)
                    deps.onElicitationRequest?.(sessionId, request, respond)
                    if (!deps.onElicitationRequest) cancel()
                })
            }
        },
        cancelPendingPermissions,
        recordUserPrompt,
        dropSession
    }
}

export function createAcpConnection(deps: AcpConnectionDeps = {}): AgentConnection {
    const sessionRegistry = new Map<string, SessionMeta>()
    const sessionConfigOptions = new Map<string, SessionConfigOption[]>()
    const configRevisions = new Map<string, number>()
    const configRequests = new Map<string, number>()
    const promptInFlight = new Set<string>()
    const runningSessions = new Set<string>()
    let nextConfigRequestToken = 0

    const replaceAuthoritativeConfig = (
        sessionId: string,
        configOptions: SessionConfigOption[]
    ): SessionConfigOption[] => {
        const next = [...configOptions]
        sessionConfigOptions.set(sessionId, next)
        configRevisions.set(sessionId, (configRevisions.get(sessionId) ?? 0) + 1)
        return next
    }

    const runtime = createAcpClientRuntime({
        ...deps,
        onSessionInfo: (sessionId, info) => {
            if (info.running) runningSessions.add(sessionId)
            else runningSessions.delete(sessionId)
            deps.onSessionInfo?.(sessionId, info)
        },
        onConfigOptions: (sessionId, configOptions) => {
            const next = replaceAuthoritativeConfig(sessionId, configOptions)
            deps.onConfigOptions?.(sessionId, next)
        }
    })
    let agent: ClientSideConnection | undefined
    let agentProcessId: string | undefined
    let initialization: Promise<ClientSideConnection> | undefined
    let stdoutController: ReadableStreamDefaultController<Uint8Array> | undefined
    let disconnectedError: Error | undefined
    let disconnectedProcessId: string | undefined
    let pendingTeardown: Promise<void> | undefined
    let authMethods: AgentAuthMethod[] = []
    let agentVersion: string | undefined
    let agentName: string | undefined
    let agentCapabilities: { loadSession: boolean; promptImage: boolean } = {
        loadSession: false,
        promptImage: false
    }
    let pendingSessionOwners = 0
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

    // 作用中連線的 listener 拆除函式。斷線路徑不一定會收到自己的 agent://exit
    // （例如 interceptor 錯誤時 process 還活著），所以 markDisconnected 必須能主動拆。
    let teardownActiveListeners: (() => void) | undefined

    const markDisconnected = (error: Error) => {
        if (agentProcessId) disconnectedProcessId = agentProcessId
        disconnectedError = error
        rejectInFlightRequests(error)
        configRequests.clear()
        promptInFlight.clear()
        runningSessions.clear()
        agentVersion = undefined
        agentName = undefined
        agent = undefined
        agentProcessId = undefined
        initialization = undefined
        const teardown = teardownActiveListeners
        teardownActiveListeners = undefined
        teardown?.()
        const controller = stdoutController
        stdoutController = undefined
        try {
            controller?.error(error)
        } catch {
            // already closed
        }
    }

    const beginTeardown = (processId: string, reason: string): Promise<void> => {
        const teardown = withTeardownTimeout(
            invoke("agent_kill", { id: processId, reason }),
            AGENT_KILL_TIMEOUT_MS
        ).catch((error) => {
            console.warn(`ACP agent teardown failed (${reason})`, error)
        })
        pendingTeardown = teardown
        void teardown.finally(() => {
            if (pendingTeardown === teardown) pendingTeardown = undefined
        })
        return teardown
    }

    const ensureConnection = (cwd: string) => {
        if (agent) return Promise.resolve(agent)
        if (initialization) return initialization
        disconnectedError = undefined
        initialization = startConnection(cwd)
            .then((connection) => {
                agent = connection
                disconnectedError = undefined
                return connection
            })
            .catch(async (error) => {
                initialization = undefined
                const processId = agentProcessId ?? disconnectedProcessId
                agentProcessId = undefined
                if (disconnectedProcessId === processId) disconnectedProcessId = undefined
                // 使用者取消冷啟動時 agent_kill 已在取消路徑被 await，不再收屍一次
                if (isAgentColdStartCancelledError(error)) throw error
                // best-effort：kill 前先撈 stderr 尾段，供 timeout 診斷用
                const tail = processId
                    ? await invoke<string[]>("agent_stderr_tail", { id: processId }).catch(() => [] as string[])
                    : []
                const timeoutStage = startupTimeoutStage(error)
                if (processId) {
                    await beginTeardown(
                        processId,
                        timeoutStage ?? "init_failed"
                    )
                }
                throw timeoutStage ? startupTimeoutError(error, tail) : error
            })
        return initialization
    }

    const disposeOwnerless = async (reason: string): Promise<boolean> => {
        if (sessionRegistry.size > 0 || pendingSessionOwners > 0) return false
        if (pendingTeardown) {
            await pendingTeardown
            return true
        }
        if (initialization) {
            try {
                await initialization
            } catch {
                if (pendingTeardown) await pendingTeardown
                return false
            }
        }
        if (sessionRegistry.size > 0 || pendingSessionOwners > 0) return false
        if (pendingTeardown) {
            await pendingTeardown
            return true
        }
        const processId = agentProcessId ?? disconnectedProcessId
        if (!processId) return false
        if (agentProcessId === processId) {
            markDisconnected(new Error("ACP ownerless connection disposed"))
        }
        if (disconnectedProcessId === processId) disconnectedProcessId = undefined
        await beginTeardown(processId, reason)
        return true
    }

    // 每次連線的狀態（process id、stdout controller、listener）都是連線自持的
    // local——listener guard 若讀共享可變的 agentProcessId，在「timeout 先清 id 再
    // kill」的路徑上會永遠不匹配而洩漏，殘留 listener 還會把下一條連線的輸出
    // 重複 enqueue（JSON-RPC 全部處理兩次）。
    const startConnection = async (cwd: string) => {
        let processId: string | undefined
        let ownController: ReadableStreamDefaultController<Uint8Array> | undefined
        const input = new ReadableStream<Uint8Array>({
            start(controller) {
                ownController = controller
                stdoutController = controller
            }
        })
        const output = new WritableStream<Uint8Array>({
            async write(chunk) {
                if (!processId) throw new Error("ACP agent process is not spawned")
                try {
                    await invoke("agent_write", { id: processId, chunk: decoder.decode(chunk) })
                } catch (error) {
                    // 寫入失敗代表 agent 已斷線：立刻 reject pending 的 initialize/請求，不等 timeout
                    const failure = error instanceof Error ? error : new Error(String(error))
                    if (agentProcessId === processId) markDisconnected(failure)
                    throw failure
                }
            }
        })
        const rawStream = ndJsonStream(output, input)
        const stream = interceptUnsupportedSessionUpdates(rawStream, runtime.client, (error) => {
            markDisconnected(agentExitedError(error))
        })
        const connection = new ClientSideConnection(() => runtime.client as unknown as Client, stream)

        // 收掉自己的 stream：仍是全域作用中 controller 時一併解除引用
        const releaseOwnStream = (error?: Error) => {
            if (stdoutController === ownController) stdoutController = undefined
            try {
                if (error) ownController?.error(error)
                else ownController?.close()
            } catch {
                // already closed
            }
        }

        // #37：bootstrap 階段的終點信號。adapter 一開始講 ACP，第一個 stdout byte
        // 就會到；bunx／npm 的安裝進度走 stderr，不會誤觸這裡。
        let settleFirstStdout: ((error?: Error) => void) | undefined
        const firstStdout = new Promise<void>((resolve, reject) => {
            settleFirstStdout = (error) => (error ? reject(error) : resolve())
        })
        // 沒走冷啟動路徑時沒有人 await 它——先掛 no-op handler，避免 exit 時變成
        // unhandled rejection。
        firstStdout.catch(() => {})

        const unlistenStdout = await listen<{ id: string; line: string }>("agent://stdout", (event) => {
            if (!processId || event.payload.id !== processId) return
            settleFirstStdout?.()
            try {
                ownController?.enqueue(encoder.encode(`${event.payload.line}\n`))
            } catch {
                // stream 已關閉（連線已被淘汰）
            }
        })
        const unlistenExit = await listen<{ id: string; code: number | null; stderrTail?: string[] }>("agent://exit", (event) => {
            if (!processId || event.payload.id !== processId) return
            const exitError = agentExitedEarlyError(event.payload.code, event.payload.stderrTail ?? [])
            // bootstrap 階段就 exit（如 offline 時 bunx 安裝失敗）：立刻失敗，
            // 不必等滿冷啟動視窗
            settleFirstStdout?.(exitError)
            teardownListeners()
            if (agentProcessId === processId) {
                markDisconnected(exitError)
            } else {
                // 已被淘汰的連線（如 init timeout 後被 kill）：只收尾自己的資源，
                // 不能動到可能已建立的新連線的全域狀態
                releaseOwnStream(exitError)
            }
        })
        let torndown = false
        const teardownListeners = () => {
            if (torndown) return
            torndown = true
            // Tauri's UnlistenFn may reject in a partially torn-down test/app
            // event runtime. Listener cleanup is best-effort and must not turn a
            // prepared-only disposal into an unhandled rejection.
            try {
                void Promise.resolve(unlistenStdout()).catch(() => {})
            } catch {
                // event runtime already gone
            }
            try {
                void Promise.resolve(unlistenExit()).catch(() => {})
            } catch {
                // event runtime already gone
            }
        }

        const timing: StartupTiming = {}
        let spawnCommand = ""
        try {
            const command = typeof deps.command === "function" ? deps.command() : deps.command
            spawnCommand = await resolveAgentSpawnCommand(command ?? DEFAULT_AGENT_COMMAND)
            const spawnedAt = Date.now()
            processId = await invoke<string>("agent_spawn", { command: spawnCommand, cwd })
            agentProcessId = processId
            teardownActiveListeners = teardownListeners
            authMethods = []
            agentVersion = undefined
            agentName = undefined
            agentCapabilities = { loadSession: false, promptImage: false }
            const initializing = trackAgentRequest(() => connection.initialize({
                protocolVersion: 1,
                clientCapabilities: {
                    fs: { readTextFile: true, writeTextFile: true },
                    terminal: true,
                    session: { configOptions: { boolean: {} } },
                    // UNSTABLE capability（P3）：form elicitation。舊 agent 的 zod
                    // 對未知欄位為 strip 語意，宣告本身不影響相容性。
                    elicitation: { form: {} }
                }
            }))
            // bootstrap 階段若先失敗，錯誤由下方的 race 回報；這裡只避免 unhandled rejection。
            initializing.catch(() => {})

            // 只有「會從 registry 下載 package」的 runtime 需要冷啟動視窗；本機既有的
            // adapter（node <path>、內建 pi adapter）維持單一短 timeout，hang 住時
            // 仍在 initialize timeout 內失敗。
            if (usesRegistryRuntime(spawnCommand)) {
                const bootstrapProcessId = processId
                registerColdStart({
                    processId: bootstrapProcessId,
                    runtime: resolveCommandRuntime(spawnCommand).runtime,
                    cwd,
                    startedAt: spawnedAt,
                    cancelling: false
                }, async () => {
                    settleFirstStdout?.(coldStartCancelledError())
                    await invoke("agent_kill", {
                        id: bootstrapProcessId,
                        reason: "bootstrap_cancelled"
                    }).catch(() => undefined)
                })
                try {
                    await Promise.race([
                        withTimeout(
                            firstStdout,
                            deps.bootstrapTimeoutMs ?? BOOTSTRAP_TIMEOUT_MS,
                            "cold start"
                        ),
                        initializing.then(() => undefined)
                    ])
                } finally {
                    releaseColdStart(bootstrapProcessId)
                }
                timing.bootstrapMs = Date.now() - spawnedAt
            }

            const handshakeAt = Date.now()
            const initializeResult = await withTimeout(
                initializing,
                deps.initializeTimeoutMs ?? INITIALIZE_TIMEOUT_MS,
                "initialize"
            )
            timing.initializeMs = Date.now() - handshakeAt
            logStartupTiming("connected", spawnCommand, timing)
            authMethods = normalizeAuthMethods(asRecord(initializeResult).authMethods)
            agentCapabilities = normalizeAgentCapabilities(asRecord(initializeResult).agentCapabilities)
            const implementation = asRecord(asRecord(initializeResult).agentInfo)
            const reportedVersion = firstString(implementation.version)?.trim()
            agentVersion = reportedVersion || undefined
            agentName = firstString(implementation.name)?.trim() || undefined
            return connection
        } catch (error) {
            logStartupTiming(startupFailurePhase(error), spawnCommand, timing)
            teardownListeners()
            releaseOwnStream()
            throw error
        }
    }

    return {
        async prepare(cwd) {
            await ensureConnection(cwd)
        },
        // agentId 在單一連線實作被忽略；Phase 2 router 才會據此選 agent。
        async newSession(cwd, _agentId) {
            pendingSessionOwners += 1
            let failureKind: SessionNewFailureKind | undefined
            try {
                const connection = await ensureConnection(cwd)
                const result = await trackAuthRequired(
                    () => trackAgentRequest(() => connection.newSession({ cwd, mcpServers: [] })),
                    { cwd, sessionId: null },
                    () => authMethods
                )
                rememberSession(result.sessionId, cwd)
                const piAcp = asRecord(asRecord(asRecord(result)._meta).piAcp)
                const startupInfo = firstString(piAcp.startupInfo) ?? null
                return {
                    sessionId: result.sessionId,
                    startupInfo,
                    ...(agentVersion ? { agentVersion } : {}),
                    ...(agentName ? { agentName } : {}),
                    configOptions: replaceAuthoritativeConfig(
                        result.sessionId,
                        normalizeSessionConfigOptions(asRecord(result).configOptions)
                    )
                }
            } catch (error) {
                failureKind = classifySessionNewFailure(error, disconnectedError !== undefined)
                throw error
            } finally {
                pendingSessionOwners -= 1
                if (failureKind && failureKind !== "auth-required") {
                    void disposeOwnerless("session_new_failed").catch(() => false)
                }
            }
        },
        async loadSession(id, cwd) {
            pendingSessionOwners += 1
            try {
                const connection = await ensureConnection(cwd)
                const result = await trackAgentRequest(() => connection.loadSession({ sessionId: id, cwd, mcpServers: [] }))
                rememberSession(id, cwd)
                const piAcp = asRecord(asRecord(asRecord(result)._meta).piAcp)
                const startupInfo = firstString(piAcp.startupInfo) ?? null
                return {
                    startupInfo,
                    ...(agentVersion ? { agentVersion } : {}),
                    ...(agentName ? { agentName } : {}),
                    configOptions: replaceAuthoritativeConfig(
                        id,
                        normalizeSessionConfigOptions(asRecord(result).configOptions)
                    )
                }
            } finally {
                pendingSessionOwners -= 1
            }
        },
        async listSessions(cwd) {
            return [...sessionRegistry.values()].filter((session) => session.cwd === cwd)
        },
        async prompt(sessionId, blocks) {
            const connection = agent
            if (!connection) throw disconnectedError ?? new Error("ACP connection is not initialized")
            runtime.recordUserPrompt(sessionId, blocks)
            promptInFlight.add(sessionId)
            try {
                const response = await trackAuthRequired(
                    () => trackAgentRequest(() => connection.prompt({ sessionId, prompt: blocks })),
                    { cwd: sessionRegistry.get(sessionId)?.cwd ?? null, sessionId },
                    () => authMethods
                )
                return response.stopReason
            } finally {
                promptInFlight.delete(sessionId)
            }
        },
        async cancel(sessionId) {
            const connection = agent
            if (!connection) throw disconnectedError ?? new Error("ACP connection is not initialized")
            await connection.cancel({ sessionId })
            runtime.cancelPendingPermissions(sessionId)
        },
        async setSessionConfigOption(sessionId, configId, value) {
            const connection = agent
            if (!connection) throw disconnectedError ?? new Error("ACP connection is not initialized")
            // turn 進行中允許改 config（soak 回饋 #1/#5）；僅擋同 session 並發 setter。
            if (configRequests.has(sessionId)) {
                throw new Error("A session configuration change is already pending")
            }
            const token = ++nextConfigRequestToken
            const requestRevision = configRevisions.get(sessionId) ?? 0
            configRequests.set(sessionId, token)
            const params = typeof value === "boolean"
                ? { sessionId, configId, type: "boolean" as const, value }
                : { sessionId, configId, value }
            try {
                const result = await trackAgentRequest(() => connection.setSessionConfigOption(params))
                if (
                    configRequests.get(sessionId) !== token
                    || (configRevisions.get(sessionId) ?? 0) !== requestRevision
                ) {
                    return [...(sessionConfigOptions.get(sessionId) ?? [])]
                }
                return replaceAuthoritativeConfig(
                    sessionId,
                    normalizeSessionConfigOptions(asRecord(result).configOptions)
                )
            } finally {
                if (configRequests.get(sessionId) === token) configRequests.delete(sessionId)
            }
        },
        async disposePrepared() {
            return disposeOwnerless("prepared_dispose")
        },
        disposeOwnerless,
        pendingTeardown: () => pendingTeardown,
        processId: () => agentProcessId,
        async supportsLoadSession(cwd) {
            await ensureConnection(cwd)
            return agentCapabilities.loadSession
        },
        supportsImagePrompt() {
            return agentCapabilities.promptImage
        },
        dropSession(sessionId) {
            runtime.dropSession(sessionId)
            sessionRegistry.delete(sessionId)
            sessionConfigOptions.delete(sessionId)
            configRevisions.delete(sessionId)
            configRequests.delete(sessionId)
            promptInFlight.delete(sessionId)
            runningSessions.delete(sessionId)
        }
    }
}

function normalizeAgentCapabilities(value: unknown): { loadSession: boolean; promptImage: boolean } {
    const record = asRecord(value)
    const promptCapabilities = asRecord(record.promptCapabilities)
    return {
        loadSession: record.loadSession === true,
        promptImage: promptCapabilities.image === true
    }
}

function promptBlocksToText(blocks: PromptBlock[]): string {
    return blocks.flatMap((block) => {
        if (block.type === "text") return [block.text]
        if (block.type === "image") return ["[image]"]
        return [block.title ?? block.name]
    }).join(" ").trim()
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
    return [...transcript, { id: newEntryId(), who, text, streaming: true }]
}

function appendUserImage(
    transcript: TranscriptEntry[],
    image: { mimeType: string; dataUrl: string }
): TranscriptEntry[] {
    const last = transcript.at(-1)
    if (last && "who" in last && last.who === "you" && last.streaming) {
        return [
            ...transcript.slice(0, -1),
            { ...last, images: [...(last.images ?? []), image], streaming: true }
        ]
    }
    return [...transcript, { id: newEntryId(), who: "you", text: "", streaming: true, images: [image] }]
}

function appendThought(transcript: TranscriptEntry[], text: string): TranscriptEntry[] {
    if (!text) return transcript
    const last = transcript.at(-1)
    if (last && "kind" in last && last.kind === "thought") {
        return [...transcript.slice(0, -1), { ...last, text: `${last.text}${text}` }]
    }
    return [...transcript, { id: newEntryId(), kind: "thought", text }]
}

function enumOptionsFrom(list: unknown[]): ElicitationField["options"] {
    return list.flatMap((item) => {
        const option = asRecord(item)
        return typeof option.const === "string"
            ? [{
                value: option.const,
                label: typeof option.title === "string" ? option.title : option.const,
                ...(typeof option.description === "string" ? { description: option.description } : {})
            }]
            : []
    })
}

// form elicitation → UI 欄位清單；含不支援的欄位型別（object／未知、或 array
// 缺有效選項）時回 null，caller 以 cancel 回應（agent 端拿 default，graceful
// degradation）。array＝ACP multiselect：items.enum（字串清單）或 items.anyOf
// （titled EnumOption）；minItems/maxItems 刻意不驗（producer 是我們的 adapter，
// 不產生它們）。
function normalizeElicitationRequest(record: Record<string, unknown>): ElicitationRequest | null {
    const schema = asRecord(record.requestedSchema)
    const properties = asRecord(schema.properties)
    const required = Array.isArray(schema.required)
        ? schema.required.filter((key): key is string => typeof key === "string")
        : []
    const multiline = asRecord(asRecord(record._meta).yuzora).multiline === true
    const fields: ElicitationField[] = []
    for (const [key, raw] of Object.entries(properties)) {
        const property = asRecord(raw)
        const type = property.type
        if (type !== "string" && type !== "boolean" && type !== "number" && type !== "integer" && type !== "array") {
            return null
        }
        let options: ElicitationField["options"]
        if (type === "string") {
            if (Array.isArray(property.enum)) {
                options = property.enum
                    .filter((value): value is string => typeof value === "string")
                    .map((value) => ({ value, label: value }))
            } else if (Array.isArray(property.oneOf)) {
                options = enumOptionsFrom(property.oneOf)
            }
        } else if (type === "array") {
            const items = asRecord(property.items)
            if (Array.isArray(items.enum)) {
                options = items.enum
                    .filter((value): value is string => typeof value === "string")
                    .map((value) => ({ value, label: value }))
            } else if (Array.isArray(items.anyOf)) {
                options = enumOptionsFrom(items.anyOf)
            }
            if (!options || options.length === 0) return null
        }
        const defaultValue = property.default
        const defaultStrings = type === "array"
            && Array.isArray(defaultValue)
            && defaultValue.every((value): value is string => typeof value === "string")
            ? defaultValue
            : undefined
        fields.push({
            key,
            type,
            ...(typeof property.title === "string" ? { title: property.title } : {}),
            ...(typeof property.description === "string" ? { description: property.description } : {}),
            required: required.includes(key),
            ...(multiline && type === "string" && !options ? { multiline: true } : {}),
            ...(typeof defaultValue === "string" || typeof defaultValue === "number" || typeof defaultValue === "boolean"
                ? { defaultValue }
                : {}),
            ...(defaultStrings ? { defaultValue: defaultStrings } : {}),
            ...(options && options.length > 0 ? { options } : {})
        })
    }
    if (fields.length === 0) return null
    return {
        message: typeof record.message === "string" ? record.message : "",
        ...(typeof schema.title === "string" ? { title: schema.title } : {}),
        fields
    }
}

function permissionBlock(params: RequestPermissionRequest): BlockEntry {
    const title = params.toolCall.title ?? "Permission request"
    return {
        id: newEntryId(),
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
        if (typeof record.name !== "string" || typeof record.description !== "string") return []
        return [{
            name: record.name,
            description: record.description,
            ...optionalRecordField(record, "input"),
            ...optionalRecordField(record, "_meta")
        }]
    })
}

export function normalizeSessionConfigOptions(value: unknown): SessionConfigOption[] {
    if (!Array.isArray(value)) return []
    return value.flatMap<SessionConfigOption>((item) => {
        const record = asRecord(item)
        if (typeof record.id !== "string" || typeof record.name !== "string") return []
        const base = {
            id: record.id,
            name: record.name,
            ...optionalStringOrNullField(record, "description"),
            ...optionalStringOrNullField(record, "category"),
            ...optionalRecordField(record, "_meta")
        }
        if (record.type === "boolean" && typeof record.currentValue === "boolean") {
            return [{ ...base, type: "boolean" as const, currentValue: record.currentValue }]
        }
        if (record.type !== "select" || typeof record.currentValue !== "string") return []
        return [{
            ...base,
            type: "select" as const,
            currentValue: record.currentValue,
            options: normalizeSessionConfigSelectOptions(record.options)
        }]
    })
}

function normalizeSessionConfigSelectOptions(
    value: unknown
): SessionConfigSelectOption[] | SessionConfigSelectGroup[] {
    if (!Array.isArray(value)) return []
    const grouped = value.some((item) => typeof asRecord(item).group === "string")
    return grouped
        ? value.flatMap((item) => {
            const record = asRecord(item)
            if (typeof record.group !== "string" || typeof record.name !== "string") return []
            return [{
                group: record.group,
                name: record.name,
                options: normalizeSessionConfigSelectValues(record.options),
                ...optionalRecordField(record, "_meta")
            }]
        })
        : normalizeSessionConfigSelectValues(value)
}

function normalizeSessionConfigSelectValues(value: unknown): SessionConfigSelectOption[] {
    if (!Array.isArray(value)) return []
    return value.flatMap((item) => {
        const record = asRecord(item)
        if (typeof record.value !== "string" || typeof record.name !== "string") return []
        return [{
            value: record.value,
            name: record.name,
            ...optionalStringOrNullField(record, "description"),
            ...optionalRecordField(record, "_meta")
        }]
    })
}

function optionalStringOrNullField(
    record: Record<string, unknown>,
    key: string
): Record<string, string | null> {
    if (!(key in record)) return {}
    const value = record[key]
    return typeof value === "string" || value === null ? { [key]: value } : {}
}

function optionalRecordField(
    record: Record<string, unknown>,
    key: string
): Record<string, Record<string, unknown> | null> {
    if (!(key in record)) return {}
    const value = record[key]
    return value === null || isRecord(value) ? { [key]: value } : {}
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

function sessionTitleFromUpdate(update: unknown): string | undefined {
    const record = asRecord(update)
    if (record.sessionUpdate !== "session_info_update") return undefined
    return typeof record.title === "string" && record.title.trim() !== "" ? record.title : undefined
}

function usageFromUpdate(update: unknown): UsageInfo | undefined {
    const record = asRecord(update)
    if (record.sessionUpdate !== "usage_update") return undefined
    if (!Number.isFinite(record.used) || !Number.isFinite(record.size)) return undefined
    const cost = asRecord(record.cost)
    const hasCost = Number.isFinite(cost.amount) && typeof cost.currency === "string"
    return {
        used: record.used as number,
        size: record.size as number,
        ...(hasCost ? { cost: { amount: cost.amount as number, currency: cost.currency as string } } : {})
    }
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

function withTimeout<T>(promise: Promise<T>, ms: number, stage = "initialize"): Promise<T> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(`ACP ${stage} timed out after ${ms}ms`)), ms)
    })
    return Promise.race([promise, timeoutPromise]).finally(() => {
        if (timeout) clearTimeout(timeout)
    })
}

function withTeardownTimeout(promise: Promise<unknown>, ms: number): Promise<void> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(`ACP agent teardown timed out after ${ms}ms`)), ms)
    })
    return Promise.race([promise, timeoutPromise]).then(() => undefined).finally(() => {
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
        if (isInvalidCwdRpcError(error)) {
            throw invalidCwdError(context.cwd)
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

function isInvalidCwdRpcError(error: unknown): boolean {
    const record = asRecord(error)
    if (record.code !== ACP_INVALID_PARAMS_ERROR_CODE) return false
    const data = asRecord(record.data)
    const messages = [record.message, data.message]
        .filter((value): value is string => typeof value === "string")
        .map((value) => value.toLowerCase())
    return messages.some((text) => text.includes("cwd") || text.includes("absolute path"))
}

function invalidCwdError(cwd: string | null): Error {
    const shown = cwd ? `（${cwd}）` : ""
    return new Error(
        `ACP agent 拒絕工作目錄${shown}：cwd must be an absolute path —— 請先開啟資料夾`
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

function agentExitedEarlyError(code: number | null, stderrTail: string[] = []): Error {
    const codeText = code === null ? "未知" : String(code)
    const tail = Array.isArray(stderrTail) ? stderrTail : []
    if (tail.some((line) => line.includes("EPIPE"))) {
        return new Error(
            `ACP agent exited: agent adapter crashed (EPIPE)（exit code ${codeText}）—— 請查 Settings → Logs (source: acp)`
        )
    }
    const base = `ACP agent exited: agent 行程已結束（exit code ${codeText}）`
    const summary = summarizeStderrTail(tail)
    return new Error(summary ? `${base} —— ${summary}` : base)
}

// #37：兩段啟動的錯誤分類。cold_start_timeout＝下載／安裝階段逾時（第一個 stdout
// byte 都沒到），init_timeout＝adapter 已開口但 ACP handshake 沒完成。
function startupTimeoutStage(error: unknown): "cold_start_timeout" | "init_timeout" | null {
    if (!(error instanceof Error)) return null
    if (error.message.includes("ACP cold start timed out")) return "cold_start_timeout"
    if (error.message.includes("ACP initialize timed out")) return "init_timeout"
    return null
}

function coldStartCancelledError(): Error {
    return new Error("ACP cold start cancelled")
}

/**
 * 使用者主動取消冷啟動下載。這不是連線失敗：不進 #38 的 retry backoff，UI 也不
 * 顯示紅色錯誤 banner（見 agentRouter／agentStore）。
 */
export function isAgentColdStartCancelledError(error: unknown): boolean {
    return error instanceof Error && error.message === "ACP cold start cancelled"
}

function startupFailurePhase(error: unknown): StartupPhase {
    if (isAgentColdStartCancelledError(error)) return "bootstrap_cancelled"
    return startupTimeoutStage(error) ?? "failed"
}

function startupTimeoutError(error: unknown, stderrTail: string[]): Error {
    const base = error instanceof Error ? error.message : String(error)
    const summary = summarizeStderrTail(stderrTail)
    const hint = "see Settings → Logs (source: acp)"
    return new Error([base, summary, hint].filter(Boolean).join(" — "))
}

function summarizeStderrTail(stderrTail: string[]): string {
    if (!Array.isArray(stderrTail)) return ""
    const lines = stderrTail.slice(-2).map((line) => line.trim()).filter(Boolean)
    return lines.join(" / ")
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
            id: newEntryId(),
            kind: "error",
            text: `Unknown session update: ${sessionUpdate || "unknown"}`,
            meta: JSON.stringify({ sessionUpdate: sessionUpdate || null })
        }
    ]
}

function appendToolCall(transcript: TranscriptEntry[], update: ToolCallUpdate): TranscriptEntry[] {
    const diffs = diffStats(update.content)
    const meta = { ...toolMeta(update), ...(diffs.length > 0 ? { diffs } : {}) }
    const text = [update.title ?? "Tool call", ...toolContentText(update.content)].join("\n")
    return [
        ...transcript,
        {
            id: newEntryId(),
            kind: "tool",
            text,
            meta: serializeToolMeta(meta)
        }
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
    const addedDiffs = diffStats(update.content)
    const nextMeta: ToolMeta = {
        ...previous,
        ...toolMeta(update),
        toolCallId: update.toolCallId
    }
    if (addedDiffs.length > 0) nextMeta.diffs = [...(previous?.diffs ?? []), ...addedDiffs]
    const addedText = toolContentText(update.content)
    const nextEntry = {
        ...entry,
        text: addedText.length > 0 ? [entry.text, ...addedText].join("\n") : entry.text,
        meta: serializeToolMeta(nextMeta)
    }
    return [
        ...transcript.slice(0, index),
        nextEntry,
        ...transcript.slice(index + 1)
    ]
}

function upsertPlan(transcript: TranscriptEntry[], update: PlanUpdate): TranscriptEntry[] {
    const completed = update.entries.filter((entry) => entry.status === "completed").length
    const index = transcript.findIndex((entry) => "kind" in entry && entry.kind === "plan")
    const plan = {
        // 就地更新沿用原 id，避免 React row 重掛
        id: index === -1 ? newEntryId() : transcript[index].id,
        kind: "plan" as const,
        text: update.entries
            .map((entry) => `${planStatus(entry.status)} ${entry.content}`)
            .join("\n"),
        meta: JSON.stringify({ completed, total: update.entries.length })
    }
    if (index === -1) return [...transcript, plan]
    return [...transcript.slice(0, index), plan, ...transcript.slice(index + 1)]
}

function parseToolCallUpdate(
    record: Record<string, unknown>,
    sessionUpdate: "tool_call" | "tool_call_update"
): ToolCallUpdate | undefined {
    if (typeof record.toolCallId !== "string") return undefined
    // claude-agent-acp 對 sub-agent 內部 tool call 蓋 parentToolUseId（指向
    // spawn 它的 Agent/Task call）——保留下來供 UI 嵌套歸組。
    const parentToolCallId = asRecord(asRecord(record._meta).claudeCode).parentToolUseId
    return {
        sessionUpdate,
        toolCallId: record.toolCallId,
        ...(typeof parentToolCallId === "string" ? { parentToolCallId } : {}),
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
        // diff 內容不再另立 diff 卡（2026-07-21 使用者回饋：不需要 view/apply diff）；
        // 以佔位行併入 tool step 內文，保留「改了哪個檔」的可見性。
        if (record.type === "diff") {
            return typeof record.path === "string" ? [`[diff: ${record.path}]`] : []
        }
        return typeof record.type === "string" ? [`[${record.type}]`] : []
    })
}

function splitDiffLines(text: string): string[] {
    if (text === "") return []
    return text.replace(/\n$/, "").split("\n")
}

// 多重集合行交集近似（O(n+m)）：回答「約略新增/刪除幾行」用，不做精確 LCS——
// 被搬移的相同行會視為未變，對彙總統計可接受。oldText null＝新檔（全部視為新增）。
function diffLineStats(oldText: string | null, newText: string): { added: number; removed: number } {
    const newLines = splitDiffLines(newText)
    if (oldText === null) return { added: newLines.length, removed: 0 }
    const oldLines = splitDiffLines(oldText)
    const counts = new Map<string, number>()
    for (const line of oldLines) counts.set(line, (counts.get(line) ?? 0) + 1)
    let common = 0
    for (const line of newLines) {
        const remaining = counts.get(line) ?? 0
        if (remaining > 0) {
            common += 1
            counts.set(line, remaining - 1)
        }
    }
    return { added: newLines.length - common, removed: oldLines.length - common }
}

function diffStats(
    content: ToolContent[] | null | undefined
): { path: string; added: number; removed: number }[] {
    if (!content) return []
    return content.flatMap((item) => {
        const record = asRecord(item)
        if (record.type !== "diff" || typeof record.path !== "string" || typeof record.newText !== "string") {
            return []
        }
        const oldText = typeof record.oldText === "string" ? record.oldText : null
        return [{ path: record.path, ...diffLineStats(oldText, record.newText) }]
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
        ...(update.rawOutput !== undefined ? { rawOutput: update.rawOutput } : {}),
        ...(update.parentToolCallId !== undefined ? { parentToolCallId: update.parentToolCallId } : {})
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
