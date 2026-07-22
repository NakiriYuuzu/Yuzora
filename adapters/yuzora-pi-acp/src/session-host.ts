// Session host：一個子行程承載一個 pi AgentSession（fork by index.ts）。
// 事件翻譯逐字鏡射 pi-acp 0.0.31 的 ACP 對映（P1 contract fixtures 為準），
// UI context 結構鏡射 pi 官方 rpc-mode——關鍵差異：custom() 不回 undefined
// （RPC 模式如此，造成 pi-questions 永久 pending 的卡死）。P4 之後：
// pi-questions 的 question 走 event bridge（見 pi-questions bridge 區塊）真答題；
// 其餘 custom 即時 throw fail-fast，extension 自行收斂成 cancelled。
import { readFileSync, realpathSync } from "node:fs"
import { homedir } from "node:os"
import { isAbsolute, join, resolve as resolvePath } from "node:path"

import { loadPiSdk, resolvePiPackageRoot, TESTED_PI_VERSION_PREFIX, type PiAgentSession } from "./pi-sdk.js"
import {
    asRecord,
    type AcpPermissionOption,
    type ElicitationAction,
    type HostToParent,
    type ParentToHost
} from "./protocol.js"

function send(message: HostToParent) {
    process.send?.(message)
}

function sendUpdate(update: Record<string, unknown>) {
    send({ t: "update", update })
}

function log(message: string) {
    send({ t: "log", message })
}

// ---------------------------------------------------------------------------
// pi-acp 純函式 helpers（逐字移植 dist 0.0.31）
// ---------------------------------------------------------------------------

function getToolPath(args: unknown): string | undefined {
    const record = asRecord(args)
    if (typeof record.path === "string") return record.path
    if (typeof record.file_path === "string") return record.file_path
    return undefined
}

function getEditOldTexts(args: unknown): string[] {
    const record = asRecord(args)
    const oldTexts: string[] = []
    if (typeof record.oldText === "string") oldTexts.push(record.oldText)
    let edits: unknown = record.edits
    if (typeof edits === "string") {
        try { edits = JSON.parse(edits) } catch { edits = undefined }
    }
    if (Array.isArray(edits)) {
        for (const edit of edits) {
            const oldText = asRecord(edit).oldText
            if (typeof oldText === "string" && !oldTexts.includes(oldText)) oldTexts.push(oldText)
        }
    }
    return oldTexts
}

function findUniqueLineNumber(text: string, needle: string): number | undefined {
    if (!needle) return undefined
    const first = text.indexOf(needle)
    if (first < 0) return undefined
    if (text.indexOf(needle, first + needle.length) >= 0) return undefined
    let line = 1
    for (let index = 0; index < first; index += 1) {
        if (text.charCodeAt(index) === 10) line += 1
    }
    return line
}

function toToolCallLocations(
    args: unknown,
    cwd: string,
    line?: number
): { path: string; line?: number }[] | undefined {
    const path = getToolPath(args)
    if (!path) return undefined
    const resolved = isAbsolute(path) ? path : resolvePath(cwd, path)
    return [{ path: resolved, ...(typeof line === "number" ? { line } : {}) }]
}

function toToolKind(toolName: string): string {
    switch (toolName) {
        case "read": return "read"
        case "write":
        case "edit": return "edit"
        default: return "other"
    }
}

function toolResultToText(result: unknown): string {
    if (!result) return ""
    const record = asRecord(result)
    const details = asRecord(record.details)
    const diff = details.diff
    if (typeof diff === "string" && diff.trim()) return diff
    const content = record.content
    if (Array.isArray(content)) {
        const texts = content
            .map((item) => {
                const block = asRecord(item)
                return block.type === "text" && typeof block.text === "string" ? block.text : ""
            })
            .filter(Boolean)
        if (texts.length) return texts.join("")
    }
    const stdout = firstString(details.stdout, record.stdout, details.output, record.output)
    const stderr = firstString(details.stderr, record.stderr)
    const exitCode = firstNumber(details.exitCode, record.exitCode, details.code, record.code)
    if ((stdout && stdout.trim()) || (stderr && stderr.trim())) {
        const parts: string[] = []
        if (stdout && stdout.trim()) parts.push(stdout)
        if (stderr && stderr.trim()) parts.push(`stderr:\n${stderr}`)
        if (typeof exitCode === "number") parts.push(`exit code: ${exitCode}`)
        return parts.join("\n\n").trimEnd()
    }
    try { return JSON.stringify(result, null, 2) } catch { return String(result) }
}

function firstString(...values: unknown[]): string | undefined {
    for (const value of values) if (typeof value === "string") return value
    return undefined
}

function firstNumber(...values: unknown[]): number | undefined {
    for (const value of values) if (typeof value === "number") return value
    return undefined
}

function normalizePiMessageText(content: unknown): string {
    if (typeof content === "string") return content
    if (!Array.isArray(content)) return ""
    return content
        .map((item) => {
            const block = asRecord(item)
            return block.type === "text" && typeof block.text === "string" ? block.text : ""
        })
        .filter(Boolean)
        .join("")
}

// ---------------------------------------------------------------------------
// config options（形狀鏡射 pi-acp buildConfigOptions）
// ---------------------------------------------------------------------------

const MODEL_CONFIG_ID = "model"
const THOUGHT_LEVEL_CONFIG_ID = "thought_level"
// fallback 清單（含 max——soak 回饋 #2：社群 pi-acp hardcode 到 xhigh 是缺陷，
// 驗證一律以 session.getAvailableThinkingLevels() 的動態清單為準）。
const ALL_THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]

function availableThinkingLevels(session: PiAgentSession): string[] {
    try {
        const levels = session.getAvailableThinkingLevels()
        if (Array.isArray(levels) && levels.length > 0) return levels.map(String)
    } catch { /* fallthrough */ }
    return ALL_THINKING_LEVELS
}

interface SessionConfiguration {
    configOptions: unknown[]
    models: unknown
    modes: unknown
}

async function getSessionConfiguration(session: PiAgentSession): Promise<SessionConfiguration> {
    let available: { provider: string; id: string; name?: string }[] = []
    try {
        available = await session.modelRuntime.getAvailable()
    } catch {
        available = []
    }
    const availableModels = available
        .map((model) => {
            const provider = String(model.provider ?? "").trim()
            const id = String(model.id ?? "").trim()
            if (!provider || !id) return null
            return {
                modelId: `${provider}/${id}`,
                name: `${provider}/${String(model.name ?? id)}`,
                description: null
            }
        })
        .filter((value): value is { modelId: string; name: string; description: null } => value !== null)
    let currentModelId: string | null = null
    if (session.model) {
        const provider = String(session.model.provider ?? "").trim()
        const id = String(session.model.id ?? "").trim()
        if (provider && id) currentModelId = `${provider}/${id}`
    }
    const models = availableModels.length || currentModelId
        ? {
            availableModels,
            currentModelId: currentModelId ?? availableModels[0]?.modelId ?? "default"
        }
        : null

    const levels = availableThinkingLevels(session)
    const modes = {
        currentModeId: session.thinkingLevel ?? "medium",
        availableModes: levels.map((id) => ({ id, name: `Thinking: ${id}`, description: null }))
    }

    const configOptions: unknown[] = [
        {
            type: "select",
            id: THOUGHT_LEVEL_CONFIG_ID,
            category: "thought_level",
            name: "Thinking",
            description: "Set the reasoning effort for this session",
            currentValue: modes.currentModeId,
            options: modes.availableModes.map((mode) => ({
                value: mode.id,
                name: mode.name,
                description: mode.description
            }))
        }
    ]
    if (models?.availableModels.length) {
        configOptions.unshift({
            type: "select",
            id: MODEL_CONFIG_ID,
            category: "model",
            name: "Model",
            description: "Select the model for this session",
            currentValue: models.currentModelId,
            options: models.availableModels.map((model) => ({
                value: model.modelId,
                name: model.name,
                description: model.description
            }))
        })
    }
    return { configOptions, models, modes }
}

// 內建 slash commands（soak 回饋 #6）：名單與語意鏡射社群 pi-acp 0.0.31 的
// builtinAvailableCommands＋prompt 攔截，但改以 SDK 呼叫實作（不經 pi RPC）。
const BUILTIN_COMMANDS: { name: string; description: string }[] = [
    { name: "compact", description: "Manually compact the session context" },
    { name: "autocompact", description: "Toggle automatic context compaction" },
    { name: "export", description: "Export session to an HTML file in the session cwd" },
    { name: "session", description: "Show session stats (messages, tokens, cost, session file)" },
    { name: "name", description: "Set session display name" },
    { name: "steering", description: "Get/set pi steering message delivery mode" },
    { name: "follow-up", description: "Get/set pi follow-up message delivery mode" },
    { name: "changelog", description: "Show pi changelog" }
]

function buildAvailableCommands(session: PiAgentSession): { name: string; description: string }[] {
    const commands: { name: string; description: string }[] = []
    try {
        for (const command of session.extensionRunner.getRegisteredCommands()) {
            commands.push({ name: command.invocationName, description: String(command.description ?? "") })
        }
    } catch { /* extension runner 未備妥時略過 */ }
    try {
        for (const template of session.promptTemplates) {
            commands.push({ name: template.name, description: String(template.description ?? "") })
        }
    } catch { /* noop */ }
    try {
        for (const skill of session.resourceLoader.getSkills().skills) {
            commands.push({ name: `skill:${skill.name}`, description: String(skill.description ?? "") })
        }
    } catch { /* noop */ }
    const seen = new Set(commands.map((command) => command.name))
    for (const command of BUILTIN_COMMANDS) {
        if (!seen.has(command.name)) commands.push(command)
    }
    return commands
}

// context/pricing（soak 回饋 #3）：以 yuzora 既有的 usage_update 通道回報——
// used/size＝context tokens/window、cost＝session 累計成本（pi 記帳單位為 USD）。
function emitUsage(session: PiAgentSession) {
    try {
        const stats = session.getSessionStats()
        const usage = stats.contextUsage ?? session.getContextUsage()
        if (!usage || typeof usage.tokens !== "number" || !Number.isFinite(usage.contextWindow)) return
        const cost = Number.isFinite(stats.cost) ? stats.cost : null
        sendUpdate({
            sessionUpdate: "usage_update",
            used: usage.tokens,
            size: usage.contextWindow,
            ...(cost !== null ? { cost: { amount: cost, currency: "USD" } } : {})
        })
    } catch { /* stats 未備妥（如空 session）就不發 */ }
}

// ---------------------------------------------------------------------------
// extension UI context（結構鏡射 rpc-mode；select/confirm 走 ACP permission）
// ---------------------------------------------------------------------------

interface DialogWaiter {
    resolve: (optionId: string | null) => void
}

interface ElicitWaiter {
    resolve: (answer: { action: ElicitationAction; content: Record<string, unknown> }) => void
}

const pendingDialogs = new Map<number, DialogWaiter>()
const pendingElicits = new Map<number, ElicitWaiter>()
let nextDialogId = 1

const CONFIRM_OPTIONS: AcpPermissionOption[] = [
    { optionId: "yes", name: "Yes", kind: "allow_once" },
    { optionId: "no", name: "No", kind: "reject_once" }
]
const CHOICE_OPTION_PREFIX = "choice-"

function requestDialog(
    options: AcpPermissionOption[],
    toolCall: Record<string, unknown>,
    opts?: { signal?: AbortSignal; timeout?: number }
): Promise<string | null> {
    if (opts?.signal?.aborted) return Promise.resolve(null)
    const reqId = nextDialogId++
    return new Promise((resolve) => {
        let timeoutId: ReturnType<typeof setTimeout> | undefined
        const cleanup = () => {
            if (timeoutId) clearTimeout(timeoutId)
            opts?.signal?.removeEventListener("abort", onAbort)
            pendingDialogs.delete(reqId)
        }
        const onAbort = () => { cleanup(); resolve(null) }
        opts?.signal?.addEventListener("abort", onAbort, { once: true })
        if (opts?.timeout) timeoutId = setTimeout(() => { cleanup(); resolve(null) }, opts.timeout)
        pendingDialogs.set(reqId, {
            resolve: (optionId) => { cleanup(); resolve(optionId) }
        })
        send({ t: "dialog", reqId, options, toolCall })
    })
}

function extensionUiToolCall(method: string, title: string | undefined, rawInput: Record<string, unknown>): Record<string, unknown> {
    return {
        toolCallId: `pi-ui-${nextDialogId}`,
        title: title ?? `Pi ${method}`,
        kind: "other",
        status: "pending",
        rawInput: { method, ...rawInput }
    }
}

// ACP form elicitation（P3）：client 有 elicitation.form capability 時，
// select/confirm/input/editor 直接走結構化表單。abort/timeout 語意同 dialog
// （resolve default 並拋棄 late answer）。
function requestElicit(
    message: string,
    requestedSchema: Record<string, unknown>,
    meta: Record<string, unknown> | undefined,
    opts?: { signal?: AbortSignal; timeout?: number }
): Promise<{ action: ElicitationAction; content: Record<string, unknown> }> {
    const cancelled = { action: "cancel" as const, content: {} }
    if (opts?.signal?.aborted) return Promise.resolve(cancelled)
    const reqId = nextDialogId++
    return new Promise((resolve) => {
        let timeoutId: ReturnType<typeof setTimeout> | undefined
        const cleanup = () => {
            if (timeoutId) clearTimeout(timeoutId)
            opts?.signal?.removeEventListener("abort", onAbort)
            pendingElicits.delete(reqId)
        }
        const onAbort = () => { cleanup(); resolve(cancelled) }
        opts?.signal?.addEventListener("abort", onAbort, { once: true })
        if (opts?.timeout) timeoutId = setTimeout(() => { cleanup(); resolve(cancelled) }, opts.timeout)
        pendingElicits.set(reqId, {
            resolve: (answer) => { cleanup(); resolve(answer) }
        })
        send({ t: "elicit", reqId, message, requestedSchema, ...(meta ? { meta } : {}) })
    })
}

function notice(text: string) {
    sendUpdate({ sessionUpdate: "agent_message_chunk", content: { type: "text", text } })
}

// ---------------------------------------------------------------------------
// pi-questions bridge（P4）：question tool 的根治。pi-questions 把 service 掛在
// 全域 symbol registry（Symbol.for），host 與 extensions 同 process，直接
// subscribe「opened」事件 → 組多題 form elicitation → reply/reject 回填。
// 不注入 extension、不 exclude tool——schema/prompt 都是 pi-questions 原生的。
// ---------------------------------------------------------------------------

const QUESTIONS_SERVICE_SYMBOL = Symbol.for("vstack.pi-questions.service")

interface QuestionServiceLike {
    subscribe(listener: (event: unknown) => void): () => void
    reply(requestId: string, answers: unknown, source?: string): boolean
    reject(requestId: string, source?: string): boolean
}

interface BridgedQuestionTab {
    question: string
    options: { label: string; description: string }[]
    multiple: boolean
    customLabel: string
    customPlaceholder: string
}

interface BridgedQuestionRequest {
    header: string
    questions: BridgedQuestionTab[]
}

// 已橋接、尚未 complete 的 requestIds。custom() 以「集合非空」判定呼叫者是
// pi-questions 的 question UI（opened 事件的 publish 與 openQuestionUi 的
// custom() 呼叫在 service.ask 的同一個同步序內）。
const activeQuestionIds = new Set<string>()

function lookupQuestionService(): QuestionServiceLike | undefined {
    const candidate = (globalThis as Record<PropertyKey, unknown>)[QUESTIONS_SERVICE_SYMBOL]
    const record = asRecord(candidate)
    if (
        typeof record.subscribe === "function"
        && typeof record.reply === "function"
        && typeof record.reject === "function"
    ) {
        return candidate as unknown as QuestionServiceLike
    }
    return undefined
}

// event.request 由 pi-questions normalizeRequest 保證形狀；仍防禦性收窄。
function parseQuestionRequest(raw: unknown): BridgedQuestionRequest | null {
    const record = asRecord(raw)
    if (!Array.isArray(record.questions) || record.questions.length === 0) return null
    const questions: BridgedQuestionTab[] = []
    for (const rawTab of record.questions) {
        const tab = asRecord(rawTab)
        if (typeof tab.question !== "string" || !Array.isArray(tab.options)) return null
        const options = tab.options.flatMap((rawOption) => {
            const option = asRecord(rawOption)
            return typeof option.label === "string"
                ? [{ label: option.label, description: typeof option.description === "string" ? option.description : "" }]
                : []
        })
        if (options.length === 0) return null
        questions.push({
            question: tab.question,
            options,
            multiple: tab.multiple === true,
            customLabel: typeof tab.customLabel === "string" ? tab.customLabel : "Something else",
            customPlaceholder: typeof tab.customPlaceholder === "string" ? tab.customPlaceholder : ""
        })
    }
    return {
        header: typeof record.header === "string" ? record.header : "Question",
        questions
    }
}

// 每題兩個欄位：q<i>＝選項（multiple → ACP array multiselect）、q<i>custom＝
// 自由文字 fallback（pi-questions 的「Something else」列，永遠存在）。
// 皆非 required——pi-questions 允許空答提交。
function questionElicitSchema(request: BridgedQuestionRequest): Record<string, unknown> {
    const properties: Record<string, unknown> = {}
    request.questions.forEach((tab, index) => {
        const enumOptions = tab.options.map((option) => ({
            const: option.label,
            title: option.label,
            ...(option.description ? { description: option.description } : {})
        }))
        properties[`q${index}`] = tab.multiple
            ? { type: "array", title: tab.question, items: { anyOf: enumOptions } }
            : { type: "string", title: tab.question, oneOf: enumOptions }
        properties[`q${index}custom`] = {
            type: "string",
            title: tab.customLabel,
            ...(tab.customPlaceholder ? { description: tab.customPlaceholder } : {})
        }
    })
    return { type: "object", title: request.header, properties, required: [] }
}

// 鏡射 pi-questions TUI 的答案組裝語意：單選＝custom 蓋過選項、多選＝custom 附加。
function questionAnswers(request: BridgedQuestionRequest, content: Record<string, unknown>): string[][] {
    return request.questions.map((tab, index) => {
        const labels = new Set(tab.options.map((option) => option.label))
        const rawCustom = content[`q${index}custom`]
        const custom = typeof rawCustom === "string" ? rawCustom.trim() : ""
        const rawChoice = content[`q${index}`]
        if (tab.multiple) {
            const selected = Array.isArray(rawChoice)
                ? rawChoice.filter((value): value is string => typeof value === "string" && labels.has(value))
                : []
            return custom ? [...selected, custom] : selected
        }
        const selected = typeof rawChoice === "string" && labels.has(rawChoice) ? [rawChoice] : []
        return custom ? [custom] : selected
    })
}

function bridgeQuestionEvent(service: QuestionServiceLike, event: unknown) {
    const record = asRecord(event)
    const requestId = typeof record.requestId === "string" ? record.requestId : null
    if (!requestId) return
    if (record.action !== "opened") {
        // answered／rejected（任何來源，含 bridge 與 shutdown）＝completed。
        if (record.action === "answered" || record.action === "rejected") activeQuestionIds.delete(requestId)
        return
    }
    const request = parseQuestionRequest(record.request)
    if (!request) return
    activeQuestionIds.add(requestId)
    void (async () => {
        try {
            const answer = await requestElicit(request.header, questionElicitSchema(request), undefined)
            // 已由他處 complete（cancel／shutdown／bridge reply）→ 晚到答案作廢。
            if (!activeQuestionIds.has(requestId)) return
            if (answer.action === "accept") {
                try {
                    service.reply(requestId, questionAnswers(request, answer.content), "ui")
                    return
                } catch (error) {
                    log(`question reply failed: ${error instanceof Error ? error.message : String(error)}`)
                }
            }
            try { service.reject(requestId, "ui") } catch { /* 已 complete */ }
        } catch (error) {
            log(`question bridge failed: ${error instanceof Error ? error.message : String(error)}`)
            try { service.reject(requestId, "ui") } catch { /* 已 complete */ }
        }
    })()
}

let questionService: QuestionServiceLike | undefined

// client 有 form capability 時才啟用；否則 custom() 維持 P2 fail-fast 全貌。
function bindQuestionBridge() {
    const service = lookupQuestionService()
    if (!service) return
    questionService = service
    service.subscribe((event) => {
        try {
            bridgeQuestionEvent(service, event)
        } catch (error) {
            log(`question event handling failed: ${error instanceof Error ? error.message : String(error)}`)
        }
    })
}

// prompt 取消／host 收尾時收斂在途 question：reject → complete → uiDone →
// custom() promise resolve → pi-questions 的 modal lock 釋放，不留殭屍。
function rejectActiveQuestions() {
    if (!questionService) return
    for (const requestId of [...activeQuestionIds]) {
        try { questionService.reject(requestId, "ui") } catch { /* 已 complete */ }
        activeQuestionIds.delete(requestId)
    }
}

// custom() factory 只在 setup 階段摸 requestRender／hardware cursor；其餘
// 屬性存取一律回 no-op function（我們永遠不真正 render）。
function stubTui(): unknown {
    const base: Record<string, unknown> = {
        requestRender() {},
        getShowHardwareCursor() { return false },
        setShowHardwareCursor() {}
    }
    return new Proxy(base, {
        get(target, property) {
            if (property in target) return target[property as string]
            return () => undefined
        }
    })
}

function createUiContext(theme: unknown, formElicitation: boolean): Record<string, unknown> {
    return {
        async select(title: string, options: string[], opts?: { signal?: AbortSignal; timeout?: number }) {
            const list = Array.isArray(options) ? options.map(String) : []
            if (!list.length) return undefined
            if (formElicitation) {
                const answer = await requestElicit(
                    String(title ?? "Select an option"),
                    {
                        type: "object",
                        properties: {
                            choice: { type: "string", title: String(title ?? "Choice"), enum: list }
                        },
                        required: ["choice"]
                    },
                    undefined,
                    opts
                )
                const choice = answer.action === "accept" ? answer.content.choice : undefined
                return typeof choice === "string" && list.includes(choice) ? choice : undefined
            }
            const permissionOptions = list.map((name, index) => ({
                optionId: `${CHOICE_OPTION_PREFIX}${index}`,
                name,
                kind: "allow_once" as const
            }))
            const optionId = await requestDialog(
                permissionOptions,
                extensionUiToolCall("select", title, { options: list }),
                opts
            )
            if (optionId === null || !optionId.startsWith(CHOICE_OPTION_PREFIX)) return undefined
            const index = Number(optionId.slice(CHOICE_OPTION_PREFIX.length))
            return Number.isSafeInteger(index) ? list.at(index) : undefined
        },
        async confirm(title: string, message: string, opts?: { signal?: AbortSignal; timeout?: number }) {
            if (formElicitation) {
                const answer = await requestElicit(
                    String(title ?? "Confirm"),
                    {
                        type: "object",
                        properties: {
                            confirmed: {
                                type: "boolean",
                                title: String(title ?? "Confirm"),
                                ...(message ? { description: String(message) } : {}),
                                default: false
                            }
                        },
                        required: ["confirmed"]
                    },
                    undefined,
                    opts
                )
                return answer.action === "accept" && answer.content.confirmed === true
            }
            const optionId = await requestDialog(
                CONFIRM_OPTIONS,
                extensionUiToolCall("confirm", title, { message }),
                opts
            )
            return optionId === "yes"
        },
        async input(title: string, placeholder?: string, opts?: { signal?: AbortSignal; timeout?: number }) {
            if (formElicitation) {
                const answer = await requestElicit(
                    String(title ?? "Input"),
                    {
                        type: "object",
                        properties: {
                            value: {
                                type: "string",
                                title: String(title ?? "Value"),
                                ...(placeholder ? { description: String(placeholder) } : {})
                            }
                        },
                        required: ["value"]
                    },
                    undefined,
                    opts
                )
                const value = answer.action === "accept" ? answer.content.value : undefined
                return typeof value === "string" ? value : undefined
            }
            notice("Pi input UI request is not supported in ACP yet; cancelling it.")
            return undefined
        },
        async editor(title: string, prefill?: string) {
            if (formElicitation) {
                const answer = await requestElicit(
                    String(title ?? "Editor"),
                    {
                        type: "object",
                        properties: {
                            value: {
                                type: "string",
                                title: String(title ?? "Text"),
                                ...(prefill ? { default: String(prefill) } : {})
                            }
                        },
                        required: ["value"]
                    },
                    // 標準 schema 沒有 textarea 語意；client 以 _meta 判斷 multiline。
                    { yuzora: { multiline: true } }
                )
                const value = answer.action === "accept" ? answer.content.value : undefined
                return typeof value === "string" ? value : undefined
            }
            notice("Pi editor UI request is not supported in ACP yet; cancelling it.")
            return undefined
        },
        notify(message: string, _type?: string) {
            notice(String(message ?? "Pi notification"))
        },
        // 與 pi RPC 模式的關鍵差異：custom 不回 undefined（那會讓 pending 永久
        // 卡死）。P4 分兩路：橋接中的 question UI（activeQuestionIds 非空——
        // opened 事件與 openQuestionUi 同一同步序）→ 執行 factory 讓它把 done
        // 塞進 pending.uiDone，bridge reply/reject → complete → done 收斂；
        // 其餘 custom → 即時 throw（P2 fail-fast），extension 自行降級。
        async custom(factory: unknown): Promise<unknown> {
            if (typeof factory !== "function" || activeQuestionIds.size === 0) {
                throw new Error("yuzora-pi-acp: custom extension UI is not supported over ACP")
            }
            return await new Promise((resolve, reject) => {
                let settled = false
                let watchdog: ReturnType<typeof setInterval> | undefined
                const settle = (fn: () => void) => {
                    if (settled) return
                    settled = true
                    if (watchdog) clearInterval(watchdog)
                    fn()
                }
                const done = (result: unknown) => settle(() => resolve(result))
                // watchdog：真正的 question UI 會在 complete 時經 uiDone settle
                // （先於 activeQuestionIds 移除）；question 全部收斂後仍未 settle
                // ＝掛在 question 窗口的未知 custom → 收斂為 unsupported。
                // 保證「任何 custom 型 extension 永不永久 pending」（spec V5）。
                watchdog = setInterval(() => {
                    if (activeQuestionIds.size === 0) {
                        settle(() => reject(new Error("yuzora-pi-acp: custom extension UI is not supported over ACP")))
                    }
                }, 5000)
                watchdog.unref?.()
                try {
                    const component = (factory as (...args: unknown[]) => unknown)(stubTui(), theme, {}, done)
                    // factory 可回傳 Promise<Component>；async factory 失敗也要收斂。
                    Promise.resolve(component).catch((error: unknown) => settle(() => reject(error)))
                } catch (error) {
                    settle(() => reject(error))
                }
            })
        },
        onTerminalInput() { return () => {} },
        setStatus() {},
        setWorkingMessage() {},
        setWorkingVisible() {},
        setWorkingIndicator() {},
        setHiddenThinkingLabel() {},
        setWidget() {},
        setFooter() {},
        setHeader() {},
        setTitle() {},
        pasteToEditor() {},
        setEditorText() {},
        getEditorText() { return "" },
        addAutocompleteProvider() {},
        setEditorComponent() {},
        getEditorComponent() { return undefined },
        get theme() { return theme },
        getAllThemes() { return [] },
        getTheme() { return undefined },
        setTheme() { return { success: false, error: "Theme switching is not supported over ACP" } },
        getToolsExpanded() { return false },
        setToolsExpanded() {}
    }
}

// extensions 摸到 ctx.ui.theme 時的保底：任何方法呼叫回傳最後一個字串引數
// （theme.fg("dim", text) → text），任何屬性存取回傳字串。真 theme 由
// sdk.initTheme() 提供時優先使用。
function fallbackTheme(): unknown {
    const fn = (...args: unknown[]) => {
        for (let index = args.length - 1; index >= 0; index -= 1) {
            if (typeof args[index] === "string") return args[index]
        }
        return ""
    }
    return new Proxy(fn, {
        get(_target, property) {
            if (property === Symbol.toPrimitive || property === "toString") return () => ""
            return fn
        }
    })
}

// ---------------------------------------------------------------------------
// 事件翻譯（鏡射 pi-acp handlePiEvent）
// ---------------------------------------------------------------------------

interface HostState {
    session: PiAgentSession
    cwd: string
    currentToolCalls: Map<string, string>
    fileSnapshots: Map<string, { path: string; oldText: string | null }>
    fileMutationToolCallIds: Set<string>
    turnActive: boolean
    cancelRequested: boolean
    // turn 進行中送達的 prompt 走 pi 原生 steering（soak 回饋 #4），不再自建
    // 佇列——queueDepth 因此恆為 0（_meta 形狀維持社群相容）。
    steeringTurnIds: Set<number>
}

function emitSessionInfo(_state: HostState, running: boolean) {
    sendUpdate({
        sessionUpdate: "session_info_update",
        _meta: { piAcp: { queueDepth: 0, running } }
    })
}

function handleAgentEvent(state: HostState, event: Record<string, unknown>) {
    const type = String(event.type ?? "")
    switch (type) {
        case "message_update": {
            const ame = asRecord(event.assistantMessageEvent)
            if (ame.type === "text_delta" && typeof ame.delta === "string") {
                sendUpdate({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: ame.delta } })
                break
            }
            if (ame.type === "thinking_delta" && typeof ame.delta === "string") {
                sendUpdate({ sessionUpdate: "agent_thought_chunk", content: { type: "text", text: ame.delta } })
                break
            }
            if (ame.type === "toolcall_start" || ame.type === "toolcall_delta" || ame.type === "toolcall_end") {
                const partial = asRecord(ame.partial)
                const contentIndex = typeof ame.contentIndex === "number" ? ame.contentIndex : 0
                const content = Array.isArray(partial.content) ? partial.content : []
                const toolCall = asRecord(ame.toolCall ?? content[contentIndex])
                const toolCallId = String(toolCall.id ?? "")
                const toolName = String(toolCall.name ?? "tool")
                if (!toolCallId) break
                let rawInput: Record<string, unknown> | undefined
                if (toolCall.arguments && typeof toolCall.arguments === "object") {
                    rawInput = toolCall.arguments as Record<string, unknown>
                } else {
                    const partialArgs = String(toolCall.partialArgs ?? "")
                    if (partialArgs) {
                        try { rawInput = JSON.parse(partialArgs) as Record<string, unknown> }
                        catch { rawInput = { partialArgs } }
                    }
                }
                const locations = toToolCallLocations(rawInput, state.cwd)
                if (!state.currentToolCalls.has(toolCallId)) {
                    state.currentToolCalls.set(toolCallId, "pending")
                    sendUpdate({
                        sessionUpdate: "tool_call",
                        toolCallId,
                        title: toolName,
                        kind: toToolKind(toolName),
                        status: "pending",
                        locations,
                        rawInput
                    })
                } else {
                    sendUpdate({
                        sessionUpdate: "tool_call_update",
                        toolCallId,
                        status: "pending",
                        locations,
                        rawInput
                    })
                }
            }
            break
        }
        case "tool_execution_start": {
            const toolCallId = String(event.toolCallId ?? "")
            if (!toolCallId) break
            const toolName = String(event.toolName ?? "tool")
            const args = event.args
            let line: number | undefined
            const isFileMutation = toolName === "edit" || toolName === "write"
            if (isFileMutation) {
                state.fileMutationToolCallIds.add(toolCallId)
                const path = getToolPath(args)
                if (path) {
                    const abs = isAbsolute(path) ? path : resolvePath(state.cwd, path)
                    try {
                        const oldText = readFileSync(abs, "utf8")
                        state.fileSnapshots.set(toolCallId, { path, oldText })
                        if (toolName === "edit") {
                            for (const needle of getEditOldTexts(args)) {
                                line = findUniqueLineNumber(oldText, needle)
                                if (typeof line === "number") break
                            }
                        }
                    } catch {
                        // 新檔：diff 的 oldText 記為 null（全檔視為新增）。
                        state.fileSnapshots.set(toolCallId, { path, oldText: null })
                    }
                }
            }
            const locations = toToolCallLocations(args, state.cwd, line)
            if (!state.currentToolCalls.has(toolCallId)) {
                state.currentToolCalls.set(toolCallId, "in_progress")
                sendUpdate({
                    sessionUpdate: "tool_call",
                    toolCallId,
                    title: toolName,
                    kind: toToolKind(toolName),
                    status: "in_progress",
                    locations,
                    rawInput: asRecord(args)
                })
            } else {
                state.currentToolCalls.set(toolCallId, "in_progress")
                sendUpdate({
                    sessionUpdate: "tool_call_update",
                    toolCallId,
                    status: "in_progress",
                    locations,
                    rawInput: asRecord(args)
                })
            }
            break
        }
        case "tool_execution_update": {
            const toolCallId = String(event.toolCallId ?? "")
            if (!toolCallId) break
            const partial = event.partialResult
            const isFileMutation = state.fileMutationToolCallIds.has(toolCallId)
            const text = isFileMutation ? "" : toolResultToText(partial)
            sendUpdate({
                sessionUpdate: "tool_call_update",
                toolCallId,
                status: "in_progress",
                content: text ? [{ type: "content", content: { type: "text", text } }] : undefined,
                ...(isFileMutation ? {} : { rawOutput: partial })
            })
            break
        }
        case "tool_execution_end": {
            const toolCallId = String(event.toolCallId ?? "")
            if (!toolCallId) break
            const result = event.result
            const isError = Boolean(event.isError)
            const text = toolResultToText(result)
            const snapshot = state.fileSnapshots.get(toolCallId)
            let content: unknown[] | undefined
            let hasStructuredDiff = false
            if (!isError && snapshot) {
                try {
                    const abs = isAbsolute(snapshot.path) ? snapshot.path : resolvePath(state.cwd, snapshot.path)
                    const newText = readFileSync(abs, "utf8")
                    if (snapshot.oldText === null || newText !== snapshot.oldText) {
                        hasStructuredDiff = true
                        content = [{ type: "diff", path: snapshot.path, oldText: snapshot.oldText, newText }]
                    }
                } catch { /* 檔案讀不回來（如被刪除）→ 落回文字內容 */ }
            }
            if (!content && !hasStructuredDiff && text) {
                content = [{ type: "content", content: { type: "text", text } }]
            }
            sendUpdate({
                sessionUpdate: "tool_call_update",
                toolCallId,
                status: isError ? "failed" : "completed",
                content,
                ...(hasStructuredDiff ? {} : { rawOutput: result })
            })
            state.currentToolCalls.delete(toolCallId)
            state.fileSnapshots.delete(toolCallId)
            state.fileMutationToolCallIds.delete(toolCallId)
            break
        }
        case "auto_retry_start": {
            const attempt = Number(event.attempt)
            const maxAttempts = Number(event.maxAttempts)
            const delayMs = Number(event.delayMs)
            let text = "Retrying..."
            if (Number.isFinite(attempt) && Number.isFinite(maxAttempts) && Number.isFinite(delayMs)) {
                let delaySeconds = Math.round(delayMs / 1000)
                if (delayMs > 0 && delaySeconds === 0) delaySeconds = 1
                text = `Retrying (attempt ${attempt}/${maxAttempts}, waiting ${delaySeconds}s)...`
            }
            notice(text)
            break
        }
        case "auto_retry_end":
            notice("Retry finished, resuming.")
            break
        case "compaction_start":
            notice("Context nearing limit, running automatic compaction...")
            break
        case "compaction_end":
            notice("Automatic compaction finished; context was summarized to continue the session.")
            emitUsage(state.session)
            break
        default:
            break
    }
}

async function ensureModelResolved(session: PiAgentSession) {
    let available: { provider: string; id: string; name?: string }[]
    try {
        available = await session.modelRuntime.getAvailable()
    } catch {
        return
    }
    if (!Array.isArray(available) || available.length === 0) return
    const current = session.model
    if (current && available.some((model) =>
        String(model.provider) === String(current.provider) && String(model.id) === String(current.id))) {
        return
    }
    let target: { provider: string; id: string } | undefined
    try {
        const settingsPath = join(homedir(), ".pi", "agent", "settings.json")
        const settings = asRecord(JSON.parse(readFileSync(settingsPath, "utf8")))
        const provider = typeof settings.defaultProvider === "string" ? settings.defaultProvider : undefined
        const modelId = typeof settings.defaultModel === "string" ? settings.defaultModel : undefined
        if (modelId) {
            target = available.find((model) =>
                (!provider || String(model.provider) === provider) && String(model.id) === modelId)
        }
    } catch { /* settings 讀不到就不補救 */ }
    if (!target) return
    try {
        await session.setModel(target)
        log(`resolved default model to ${target.provider}/${target.id} after extension load`)
    } catch (error) {
        log(`default model re-resolution failed: ${error instanceof Error ? error.message : String(error)}`)
    }
}

// ---------------------------------------------------------------------------
// lifecycle
// ---------------------------------------------------------------------------

let state: HostState | undefined

async function initialize(message: Extract<ParentToHost, { t: "init" }>) {
    const sdk = await loadPiSdk()
    if (!sdk.version.startsWith(TESTED_PI_VERSION_PREFIX)) {
        log(`pi ${sdk.version} is outside the tested range (${TESTED_PI_VERSION_PREFIX}x); proceeding anyway`)
    }

    // cwd 一律 realpath：pi 子行程（社群 pi-acp 路徑）的 process.cwd() 是實體
    // 路徑，session store 的 project 歸屬以它為 key。字串直傳會讓 macOS 的
    // /var（→/private/var）這類 symlink cwd 分裂成兩個 project 目錄——同一
    // session 兩個 runtime 就互相找不到（P5 跨 runtime 續聊實測抓到）。
    let cwd = message.cwd
    try { cwd = realpathSync(message.cwd) } catch { /* 不存在就保留原字串 */ }

    let sessionManager: unknown
    if (message.sessionFile) {
        sessionManager = sdk.SessionManager.open(message.sessionFile, undefined, cwd)
    } else if (message.loadSessionId) {
        const sessions = await sdk.SessionManager.list(cwd)
        const found = sessions.find((info) => info.id === message.loadSessionId)
        if (!found) {
            send({ t: "init-error", code: "invalid_params", message: `Unknown sessionId: ${message.loadSessionId}` })
            return
        }
        sessionManager = sdk.SessionManager.open(found.path, undefined, cwd)
    } else {
        sessionManager = sdk.SessionManager.create(cwd)
    }

    const { session } = await sdk.createAgentSession({ cwd, sessionManager })

    let theme: unknown
    try {
        theme = sdk.initTheme ? sdk.initTheme() : undefined
    } catch { theme = undefined }
    if (!theme) theme = fallbackTheme()

    await session.bindExtensions({
        uiContext: createUiContext(theme, message.formElicitation === true),
        mode: "rpc",
        onError: (error: unknown) => {
            const record = asRecord(error)
            log(`extension error [${String(record.extensionPath ?? "?")}] ${String(asRecord(record.error).message ?? record.error ?? "")}`)
        }
    })

    // default model 解析修復：SDK 路徑下 settings 的 default model 解析發生在
    // provider extension（如 router 型動態 provider）註冊之前，動態 provider 的
    // 模型會落成 unresolved placeholder。extensions 綁定後以 available 清單
    // ＋settings 重新對回；寫回相同值，不改使用者設定。
    await ensureModelResolved(session)

    // auth gate（鏡射 pi-acp：無可用模型＝未設定憑證）。
    let models: unknown[] = []
    try {
        models = await session.modelRuntime.getAvailable()
    } catch {
        models = []
    }
    if (!Array.isArray(models) || models.length === 0) {
        send({
            t: "init-error",
            code: "auth_required",
            message: "Configure an API key or log in with an OAuth provider."
        })
        session.dispose()
        return
    }

    // P4 question bridge：需要 client 的 form capability 才能出表單；沒有時
    // 不 subscribe，custom() 維持 P2 fail-fast（question → cancelled → 文字降級）。
    if (message.formElicitation === true) bindQuestionBridge()

    state = {
        session,
        cwd,
        currentToolCalls: new Map(),
        fileSnapshots: new Map(),
        fileMutationToolCallIds: new Set(),
        turnActive: false,
        cancelRequested: false,
        steeringTurnIds: new Set()
    }
    session.subscribe((event) => {
        try {
            handleAgentEvent(state!, event)
        } catch (error) {
            log(`event translation failed: ${error instanceof Error ? error.message : String(error)}`)
        }
    })

    const configuration = await getSessionConfiguration(session)
    const modelLabel = session.model ? `${session.model.provider}/${session.model.id}` : "no model"
    const startupInfo = message.loadSessionId || message.sessionFile
        ? null
        : `pi ${sdk.version} · ${modelLabel} · thinking: ${session.thinkingLevel}`
    pendingStartupInfo = startupInfo
    send({
        t: "ready",
        sessionId: session.sessionId,
        sessionFile: session.sessionFile ?? null,
        configOptions: configuration.configOptions,
        models: configuration.models,
        modes: configuration.modes,
        startupInfo
    })
}

// 內建 slash command 的執行（鏡射社群 pi-acp 的輸出格式；一律回 end_turn、
// 不進 LLM turn）。回傳 false＝非內建命令，走一般 prompt。
async function handleBuiltinCommand(current: HostState, id: number, trimmed: string): Promise<boolean> {
    const space = trimmed.indexOf(" ")
    const cmd = space === -1 ? trimmed.slice(1) : trimmed.slice(1, space)
    const argsText = space === -1 ? "" : trimmed.slice(space + 1).trim()
    const finish = () => send({ t: "result", id, ok: true, data: { stopReason: "end_turn" } })
    try {
        switch (cmd) {
            case "compact": {
                const result = await current.session.compact(argsText || undefined)
                const header = [
                    `Compaction completed.${argsText ? " (custom instructions applied)" : ""}`,
                    Number.isFinite(result.tokensBefore) ? `Tokens before: ${result.tokensBefore}` : null
                ].filter(Boolean).join("\n")
                notice(result.summary ? `${header}\n\n${result.summary}` : header)
                emitUsage(current.session)
                finish()
                return true
            }
            case "session": {
                const stats = current.session.getSessionStats()
                const lines = [
                    `Session: ${stats.sessionId}`,
                    stats.sessionFile ? `Session file: ${stats.sessionFile}` : null,
                    `Messages: ${stats.totalMessages}`,
                    `Cost: ${stats.cost}`,
                    `Tokens: in ${stats.tokens.input}, out ${stats.tokens.output}, cache read ${stats.tokens.cacheRead}, cache write ${stats.tokens.cacheWrite}, total ${stats.tokens.total}`
                ].filter((line): line is string => line !== null)
                notice(lines.join("\n"))
                finish()
                return true
            }
            case "name": {
                if (!argsText) {
                    notice("Usage: /name <name>")
                    finish()
                    return true
                }
                current.session.setSessionName(argsText)
                sendUpdate({ sessionUpdate: "session_info_update", title: argsText, updatedAt: new Date().toISOString() })
                notice(`Session name set: ${argsText}`)
                finish()
                return true
            }
            case "steering":
            case "follow-up": {
                const isSteering = cmd === "steering"
                const mode = argsText.toLowerCase()
                if (!mode) {
                    notice(`${isSteering ? "Steering" : "Follow-up"} mode: ${isSteering ? current.session.steeringMode : current.session.followUpMode}`)
                } else if (mode !== "all" && mode !== "one-at-a-time") {
                    notice(`Usage: /${cmd} all | /${cmd} one-at-a-time`)
                } else {
                    if (isSteering) current.session.setSteeringMode(mode)
                    else current.session.setFollowUpMode(mode)
                    notice(`${isSteering ? "Steering" : "Follow-up"} mode set to: ${mode}`)
                }
                finish()
                return true
            }
            case "autocompact": {
                const arg = argsText.toLowerCase()
                const enabled = arg === "on" ? true : arg === "off" ? false : !current.session.autoCompactionEnabled
                current.session.setAutoCompactionEnabled(enabled)
                notice(`Auto-compaction ${enabled ? "enabled" : "disabled"}.`)
                finish()
                return true
            }
            case "export": {
                const path = await current.session.exportToHtml()
                notice(`Session exported: ${path}`)
                finish()
                return true
            }
            case "changelog": {
                const changelogPath = join(resolvePiPackageRoot(), "CHANGELOG.md")
                const content = readFileSync(changelogPath, "utf8")
                const lines = content.split("\n")
                notice(lines.length > 120 ? `${lines.slice(0, 120).join("\n")}\n…(truncated)` : content)
                finish()
                return true
            }
            default:
                return false
        }
    } catch (error) {
        notice(`/${cmd} failed: ${error instanceof Error ? error.message : String(error)}`)
        finish()
        return true
    }
}

function startTurn(current: HostState, turn: { id: number; text: string; images: { mimeType: string; data: string }[] }) {
    current.turnActive = true
    current.cancelRequested = false
    emitSessionInfo(current, true)
    // startup banner 在首個 turn 的 running info 之後補發——對齊 pi-acp 實測
    // 時序（其 setTimeout(0) 的 banner 總落在 prompt 的 startTurn 之後）。
    if (pendingStartupInfo) {
        notice(pendingStartupInfo)
        pendingStartupInfo = null
    }
    const images = turn.images.map((image) => ({ type: "image", mimeType: image.mimeType, data: image.data }))
    current.session
        .prompt(turn.text, { ...(images.length ? { images } : {}), source: "rpc" })
        .then(() => finishTurn(current, turn.id, current.cancelRequested ? "cancelled" : "end_turn"))
        .catch((error: unknown) => {
            if (current.cancelRequested) {
                finishTurn(current, turn.id, "cancelled")
                return
            }
            const messageText = error instanceof Error ? error.message : String(error)
            const authRequired = /api key|authenticate|logged? in|credential/i.test(messageText)
            current.turnActive = false
            emitSessionInfo(current, false)
            send({ t: "result", id: turn.id, ok: false, authRequired, message: messageText })
        })
}

// turn 進行中送達的 prompt（soak 回饋 #4）：交給 pi 原生 steering——訊息插進
// 現行 turn（依使用者的 steeringMode 設定 deliver），其 promise 與主 turn 同在
// agent run 收斂時 resolve，屆時各自回覆 ACP result。
function steerTurn(current: HostState, turn: { id: number; text: string; images: { mimeType: string; data: string }[] }) {
    const images = turn.images.map((image) => ({ type: "image", mimeType: image.mimeType, data: image.data }))
    current.steeringTurnIds.add(turn.id)
    current.session
        .prompt(turn.text, { ...(images.length ? { images } : {}), source: "rpc", streamingBehavior: "steer" })
        .then(() => {
            current.steeringTurnIds.delete(turn.id)
            send({ t: "result", id: turn.id, ok: true, data: { stopReason: current.cancelRequested ? "cancelled" : "end_turn" } })
        })
        .catch((error: unknown) => {
            current.steeringTurnIds.delete(turn.id)
            if (current.cancelRequested) {
                send({ t: "result", id: turn.id, ok: true, data: { stopReason: "cancelled" } })
                return
            }
            send({ t: "result", id: turn.id, ok: false, message: error instanceof Error ? error.message : String(error) })
        })
}

function finishTurn(current: HostState, id: number, stopReason: string) {
    // 順序鏡射社群 pi-acp：先廣播 running:false（與 usage），再回 prompt result——
    // client（含 recorder）可能在 result 一到就收攤，晚於 result 的通知會漏。
    emitUsage(current.session)
    current.turnActive = false
    emitSessionInfo(current, false)
    send({ t: "result", id, ok: true, data: { stopReason } })
}

async function replay(current: HostState, id: number) {
    const crypto = await import("node:crypto")
    for (const raw of current.session.messages) {
        const message = asRecord(raw)
        const role = String(message.role ?? "")
        if (role === "user") {
            const text = normalizePiMessageText(message.content)
            if (text) sendUpdate({ sessionUpdate: "user_message_chunk", content: { type: "text", text } })
        }
        if (role === "assistant") {
            const text = normalizePiMessageText(message.content)
            if (text) sendUpdate({ sessionUpdate: "agent_message_chunk", content: { type: "text", text } })
        }
        if (role === "toolResult") {
            const toolName = String(message.toolName ?? "tool")
            const toolCallId = String(message.toolCallId ?? crypto.randomUUID())
            const isError = Boolean(message.isError)
            sendUpdate({
                sessionUpdate: "tool_call",
                toolCallId,
                title: toolName,
                kind: toToolKind(toolName),
                status: "completed",
                rawInput: null,
                rawOutput: message
            })
            const text = toolResultToText(message)
            sendUpdate({
                sessionUpdate: "tool_call_update",
                toolCallId,
                status: isError ? "failed" : "completed",
                content: text ? [{ type: "content", content: { type: "text", text } }] : null,
                rawOutput: message
            })
        }
    }
    send({ t: "result", id, ok: true })
}

async function setConfig(current: HostState, id: number, configId: string, value: string) {
    if (configId === MODEL_CONFIG_ID) {
        const available = await current.session.modelRuntime.getAvailable()
        let provider: string | null = null
        let modelId: string | null = value
        if (value.includes("/")) {
            const [head, ...rest] = value.split("/")
            provider = head
            modelId = rest.join("/")
        } else {
            const found = available.find((model) => String(model.id) === value)
            if (found) { provider = String(found.provider); modelId = String(found.id) }
        }
        const model = available.find((candidate) => String(candidate.provider) === provider && String(candidate.id) === modelId)
        if (!model) {
            send({ t: "result", id, ok: false, invalidParams: true, message: `Unknown modelId: ${value}` })
            return
        }
        await current.session.setModel(model)
    } else if (configId === THOUGHT_LEVEL_CONFIG_ID) {
        // 只擋非法字串；合法等級交給 SDK 的 clamp（setThinkingLevel 依模型
        // 能力收斂到實際生效值）——soak 回饋 #2：預先用動態清單假拒絕會在
        // model metadata 與 UI 清單不同步時誤擋 max。
        if (!ALL_THINKING_LEVELS.includes(value)) {
            send({ t: "result", id, ok: false, invalidParams: true, message: `Unknown thinking level: ${value}` })
            return
        }
        current.session.setThinkingLevel(value)
        sendUpdate({ sessionUpdate: "current_mode_update", currentModeId: current.session.thinkingLevel })
    } else {
        send({ t: "result", id, ok: false, invalidParams: true, message: `Unknown config option: ${configId}` })
        return
    }
    const configuration = await getSessionConfiguration(current.session)
    sendUpdate({ sessionUpdate: "config_option_update", configOptions: configuration.configOptions })
    send({ t: "result", id, ok: true, data: { configOptions: configuration.configOptions } })
}

let announced = false
let pendingStartupInfo: string | null = null

process.on("message", (raw: ParentToHost) => {
    void (async () => {
        const message = raw
        try {
            switch (message.t) {
                case "init":
                    await initialize(message)
                    break
                case "prompt": {
                    if (!state) return
                    // 內建 slash command（#6）：不進 LLM turn、不受 turnActive 影響。
                    if (message.images.length === 0 && message.text.trimStart().startsWith("/")) {
                        const handled = await handleBuiltinCommand(state, message.id, message.text.trim())
                        if (handled) return
                    }
                    if (state.turnActive) {
                        steerTurn(state, { id: message.id, text: message.text, images: message.images })
                        return
                    }
                    startTurn(state, { id: message.id, text: message.text, images: message.images })
                    break
                }
                case "cancel":
                    if (!state) return
                    state.cancelRequested = true
                    void state.session.abort().catch(() => undefined)
                    // 在途 question 一併收斂（yuzora 端會自己 cancel elicitation，
                    // 但其他 ACP client 未必——host 側保證 modal lock 不留殭屍）。
                    rejectActiveQuestions()
                    break
                case "set-config":
                    if (!state) return
                    await setConfig(state, message.id, message.configId, message.value)
                    break
                case "replay":
                    if (!state) return
                    await replay(state, message.id)
                    break
                case "announce": {
                    if (!state || announced) return
                    announced = true
                    // banner 不在此發（見 startTurn）；commands 為 floating 廣播。
                    sendUpdate({
                        sessionUpdate: "available_commands_update",
                        availableCommands: buildAvailableCommands(state.session)
                    })
                    break
                }
                case "dialog-answer": {
                    const waiter = pendingDialogs.get(message.reqId)
                    waiter?.resolve(message.optionId)
                    break
                }
                case "elicit-answer": {
                    const waiter = pendingElicits.get(message.reqId)
                    waiter?.resolve({ action: message.action, content: message.content ?? {} })
                    break
                }
                case "shutdown":
                    state?.session.dispose()
                    process.exit(0)
            }
        } catch (error) {
            const text = error instanceof Error ? error.message : String(error)
            if (message.t === "init") send({ t: "init-error", code: "internal", message: text })
            else log(`host error handling ${message.t}: ${text}`)
        }
    })()
})

// parent 死亡（IPC channel 斷）→ 自我了斷，不留孤兒。
process.on("disconnect", () => {
    try { state?.session.dispose() } catch { /* noop */ }
    process.exit(0)
})
