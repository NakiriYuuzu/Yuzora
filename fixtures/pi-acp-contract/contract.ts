// 錄音 → contract 形狀：把 wire 訊息化約為「結構簽名」序列（忽略 id／時間／
// 文字內容／路徑／token 數），供 (1) 不變量檢查（單份錄音自身健全）與
// (2) 雙錄音結構比對（P2 builtin adapter parity gate）。內容差異（模型輸出）
// 刻意不比——契約是事件序與欄位形狀，不是語料。
import type { CapturedLine } from "./driver"

export interface Recording {
    meta: Record<string, unknown>
    lines: CapturedLine[]
    trailer?: Record<string, unknown>
}

export function parseRecording(text: string): Recording {
    const rows = text.split("\n").filter((line) => line.trim() !== "").map((line) => JSON.parse(line))
    const meta = rows[0]?.meta
    if (!meta) throw new Error("recording missing meta first line")
    const trailerRow = rows[rows.length - 1]?.trailer !== undefined ? rows[rows.length - 1] : undefined
    const body = rows.slice(1, trailerRow ? -1 : undefined)
    return {
        meta: meta as Record<string, unknown>,
        lines: body as CapturedLine[],
        ...(trailerRow ? { trailer: trailerRow.trailer as Record<string, unknown> } : {})
    }
}

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

function sortedKeys(value: unknown): string {
    return Object.keys(asRecord(value)).sort().join(",")
}

// tool content 只取型別序列（text 內容不比）。
function contentTypes(content: unknown): string {
    if (!Array.isArray(content)) return "-"
    return content.map((item) => String(asRecord(item).type ?? "?")).join("+") || "empty"
}

/** 單一 wire 訊息 → 結構簽名。requestMethodById 供 response 對回原 method。 */
export function signature(
    line: CapturedLine,
    requestMethodById: Map<string, string>
): string {
    const msg = line.msg
    const arrow = line.dir === "c2a" ? ">" : "<"
    const hasId = msg.id !== undefined && msg.id !== null
    if (hasId && typeof msg.method === "string") {
        requestMethodById.set(`${line.dir}:${String(msg.id)}`, msg.method)
        return `${arrow}req ${msg.method}`
    }
    if (hasId) {
        const from = line.dir === "c2a" ? "a2c" : "c2a"
        const method = requestMethodById.get(`${from}:${String(msg.id)}`) ?? "?"
        if (msg.error !== undefined) return `${arrow}res ${method} !error`
        if (method === "session/prompt") {
            return `${arrow}res ${method} stopReason=${String(asRecord(msg.result).stopReason ?? "-")}`
        }
        return `${arrow}res ${method} keys=${sortedKeys(msg.result) || "-"}`
    }
    if (msg.method === "session/update") {
        const update = asRecord(asRecord(msg.params).update)
        const variant = String(update.sessionUpdate ?? "?")
        if (variant === "tool_call" || variant === "tool_call_update") {
            const parts = [
                `status=${String(update.status ?? "-")}`,
                `content=${contentTypes(update.content)}`,
                `rawInput=${update.rawInput !== undefined && update.rawInput !== null ? "y" : "n"}`,
                `rawOutput=${update.rawOutput !== undefined && update.rawOutput !== null ? "y" : "n"}`,
                `locations=${Array.isArray(update.locations) && update.locations.length > 0 ? "y" : "n"}`
            ]
            if (variant === "tool_call") parts.unshift(`kind=${String(update.kind ?? "-")}`)
            return `<ntf session/update ${variant} ${parts.join(" ")}`
        }
        if (variant === "session_info_update") {
            return `<ntf session/update ${variant} metaKeys=${sortedKeys(asRecord(asRecord(update._meta).piAcp))}`
        }
        return `<ntf session/update ${variant}`
    }
    return `${arrow}ntf ${String(msg.method ?? "?")}`
}

export interface CollapsedEvent {
    sig: string
    count: number
}

/** 簽名序列＋連續相同者合併（chunk 洪流 → 一筆；比對時忽略 count）。 */
export function collapse(recording: Recording): CollapsedEvent[] {
    const requestMethodById = new Map<string, string>()
    const events: CollapsedEvent[] = []
    for (const line of recording.lines) {
        const sig = signature(line, requestMethodById)
        const previous = events[events.length - 1]
        if (previous && previous.sig === sig) previous.count += 1
        else events.push({ sig, count: 1 })
    }
    return events
}

export interface Finding {
    level: "error" | "warn"
    message: string
}

/** 單份錄音的健全性不變量（pi-acp profile）。 */
export function invariants(recording: Recording): Finding[] {
    const findings: Finding[] = []
    const lines = recording.lines
    const error = (message: string) => findings.push({ level: "error", message })
    const warn = (message: string) => findings.push({ level: "warn", message })

    const firstC2a = lines.find((line) => line.dir === "c2a")
    if (!firstC2a || asRecord(firstC2a.msg).method !== "initialize") {
        error("first client message is not initialize")
    }

    // request/response 一一對應（雙向）。
    for (const dir of ["c2a", "a2c"] as const) {
        const responseDir = dir === "c2a" ? "a2c" : "c2a"
        const requestIds = lines
            .filter((line) => line.dir === dir && line.msg.method !== undefined && line.msg.id !== undefined)
            .map((line) => String(line.msg.id))
        const responseIds = lines
            .filter((line) => line.dir === responseDir && line.msg.method === undefined && line.msg.id !== undefined)
            .map((line) => String(line.msg.id))
        for (const id of requestIds) {
            const count = responseIds.filter((candidate) => candidate === id).length
            if (count !== 1) error(`${dir} request id=${id} has ${count} responses (expected 1)`)
        }
        for (const id of responseIds) {
            if (!requestIds.includes(id)) error(`${responseDir} response id=${id} without a matching request`)
        }
    }

    // initialize 回應形狀。
    const requestMethodById = new Map<string, string>()
    for (const line of lines) signature(line, requestMethodById)
    const initResponse = lines.find(
        (line) => line.dir === "a2c" && line.msg.method === undefined
            && requestMethodById.get(`c2a:${String(line.msg.id)}`) === "initialize"
    )
    if (initResponse) {
        const result = asRecord(initResponse.msg.result)
        if (result.protocolVersion === undefined) error("initialize result missing protocolVersion")
        if (!Array.isArray(result.authMethods)) warn("initialize result missing authMethods array")
    }

    // session/update 的 sessionId 必須屬於本錄音建立/載入的 session。
    const knownSessions = new Set<string>()
    for (const line of lines) {
        if (line.dir === "a2c" && line.msg.method === undefined) {
            const sessionId = asRecord(line.msg.result).sessionId
            if (typeof sessionId === "string") knownSessions.add(sessionId)
        }
        if (line.dir === "c2a" && line.msg.method === "session/load") {
            const sessionId = asRecord(line.msg.params).sessionId
            if (typeof sessionId === "string") knownSessions.add(sessionId)
        }
    }
    for (const line of lines) {
        if (line.dir === "a2c" && line.msg.method === "session/update") {
            const sessionId = asRecord(line.msg.params).sessionId
            if (typeof sessionId === "string" && knownSessions.size > 0 && !knownSessions.has(sessionId)) {
                error(`session/update for unknown sessionId ${sessionId}`)
            }
        }
    }

    // 每個 prompt 回應都要有 stopReason；每個 toolCallId 要在對應 prompt 回應前收斂。
    const promptResponses = lines.filter(
        (line) => line.dir === "a2c" && line.msg.method === undefined
            && requestMethodById.get(`c2a:${String(line.msg.id)}`) === "session/prompt"
    )
    for (const response of promptResponses) {
        if (response.msg.error !== undefined) continue
        if (asRecord(response.msg.result).stopReason === undefined) {
            error(`session/prompt response id=${String(response.msg.id)} missing stopReason`)
        }
    }
    const terminalStatus = new Map<string, string>()
    for (const line of lines) {
        if (line.dir !== "a2c" || line.msg.method !== "session/update") continue
        const update = asRecord(asRecord(line.msg.params).update)
        const variant = update.sessionUpdate
        if (variant !== "tool_call" && variant !== "tool_call_update") continue
        const toolCallId = String(update.toolCallId ?? "")
        if (toolCallId === "") { error(`${String(variant)} without toolCallId`); continue }
        const status = typeof update.status === "string" ? update.status : terminalStatus.get(toolCallId) ?? "-"
        terminalStatus.set(toolCallId, status)
    }
    for (const [toolCallId, status] of terminalStatus) {
        if (status !== "completed" && status !== "failed") {
            const cancelled = lines.some((line) => line.dir === "c2a" && line.msg.method === "session/cancel")
            const finding = `toolCallId ${toolCallId} never reached completed/failed (last=${status})`
            if (cancelled) warn(`${finding} — cancel scenario, acceptable`)
            else error(finding)
        }
    }

    return findings
}

export interface CompareResult {
    equal: boolean
    report: string[]
}

// 位置不構成契約的「浮動」簽名：available_commands_update 是 async best-effort
// 廣播（pi-acp 以 setTimeout 0 fire-and-forget 送出），與其他通知的相對順序
// 隨時序漂移。比對時抽出、只驗出現次數相等。
const FLOATING_PATTERNS = [/available_commands_update/]

// additive 簽名（記錄在案的豁免，2026-07-22）：builtin adapter 對社群基線的
// 增量通知——usage_update（context/pricing 通道；社群 0.0.31 沒有）。比對前
// 從兩側剔除、僅列入報告，不構成分歧。
const ADDITIVE_PATTERNS = [/usage_update/]

function isFloating(sig: string): boolean {
    return FLOATING_PATTERNS.some((pattern) => pattern.test(sig))
}

function isAdditive(sig: string): boolean {
    return ADDITIVE_PATTERNS.some((pattern) => pattern.test(sig))
}

/** 兩份錄音的 collapsed 簽名序列逐項比對；差異列出首個分歧點±3 context。 */
export function compare(a: Recording, b: Recording): CompareResult {
    const collapsedA = collapse(a)
    const collapsedB = collapse(b)
    const additiveCount = collapsedA.filter((event) => isAdditive(event.sig)).length
        + collapsedB.filter((event) => isAdditive(event.sig)).length
    const allA = collapsedA.filter((event) => !isAdditive(event.sig))
    const allB = collapsedB.filter((event) => !isAdditive(event.sig))
    const floatingA = allA.filter((event) => isFloating(event.sig))
    const floatingB = allB.filter((event) => isFloating(event.sig))
    const eventsA = allA.filter((event) => !isFloating(event.sig))
    const eventsB = allB.filter((event) => !isFloating(event.sig))
    const report: string[] = []
    if (additiveCount > 0) report.push(`additive signatures excluded: ${additiveCount} (usage_update)`)
    const floatCountA = floatingA.reduce((sum, event) => sum + event.count, 0)
    const floatCountB = floatingB.reduce((sum, event) => sum + event.count, 0)
    if (floatCountA !== floatCountB) {
        report.push(`floating signature count mismatch: A=${floatCountA} B=${floatCountB} (available_commands_update)`)
        return { equal: false, report }
    }
    if (floatCountA > 0) report.push(`floating signatures matched out-of-order: ${floatCountA} (available_commands_update)`)

    // chunk-slide 豁免：startup banner（agent_message_chunk）落點是跨 process
    // 時序（pi-acp 以 setTimeout 0 送出），與 info/current_mode 的相對順序在兩份
    // 錄音間可漂移。分歧點若任一側是純 agent_message_chunk，允許跳過該事件
    // 續比（上限 4 次、記入報告）；其餘簽名維持嚴格序。
    const CHUNK_SIG = "<ntf session/update agent_message_chunk"
    let indexA = 0
    let indexB = 0
    let slides = 0
    while (indexA < eventsA.length || indexB < eventsB.length) {
        const sigA = eventsA[indexA]?.sig
        const sigB = eventsB[indexB]?.sig
        if (sigA === sigB) { indexA += 1; indexB += 1; continue }
        if (slides < 4 && sigA === CHUNK_SIG) { slides += 1; indexA += 1; continue }
        if (slides < 4 && sigB === CHUNK_SIG) { slides += 1; indexB += 1; continue }
        report.push(`divergence at collapsed event A#${indexA} / B#${indexB}:`)
        for (let offset = -3; offset <= 3; offset += 1) {
            const marker = offset === 0 ? ">>" : "  "
            const posA = indexA + offset
            const posB = indexB + offset
            if (posA >= 0 || posB >= 0) {
                report.push(`${marker} A[${posA}] ${eventsA[posA]?.sig ?? "(absent)"}`)
                report.push(`${marker} B[${posB}] ${eventsB[posB]?.sig ?? "(absent)"}`)
            }
        }
        return { equal: false, report }
    }
    if (slides > 0) report.push(`chunk-slide exemptions used: ${slides} (startup banner timing)`)
    report.push(`identical contract shape (${eventsA.length}/${eventsB.length} collapsed events)`)
    return { equal: true, report }
}
