// Session Index persistence（ADR 0001）：本地只存索引欄位，不存 transcript。
// 慣例沿 settingsStorage.ts（相同的 localStorage key 前綴、JSON 序列化、容錯讀取），
// 但索引需要「每 cwd 上限＋淘汰最舊」的額外邏輯，故獨立成自己的模組。

export interface SessionIndexEntry {
    sessionId: string
    cwd: string
    agentId?: string
    /** One-way routing identity for custom agents; never the raw command. */
    customCommandFingerprint?: string
    agentTitle?: string
    sessionAlias?: string | null
    derivedTitle?: string
    createdAt: number
    lastActiveAt: number
}

interface SessionIndexEnvelope {
    version: 1
    entries: SessionIndexEntry[]
}

export const SESSION_INDEX_STORAGE_KEY = "yuzora:agent-sessions"
const SESSION_INDEX_VERSION = 1
const MAX_ENTRIES_PER_CWD = 20

// F12：與 state/recentWorkspaces.ts 的 normalizeWorkspacePath 同款邏輯，複製一份
// 最小實作到 storage 層，避免從 storage 反向依賴上層 state module。「/ws」與
// 「/ws/」視為同一個 cwd 桶，合計仍受 MAX_ENTRIES_PER_CWD 上限約束。
function normalizeCwd(path: string): string {
    const stripped = path.replace(/[/\\]+$/, "")
    return stripped === "" ? "/" : stripped
}

export function loadSessionIndex(): SessionIndexEntry[] {
    let raw: string | null
    try {
        raw = localStorage.getItem(SESSION_INDEX_STORAGE_KEY)
    } catch {
        return []
    }
    if (!raw) return []
    try {
        const parsed = JSON.parse(raw) as Partial<SessionIndexEnvelope>
        if (parsed.version !== SESSION_INDEX_VERSION || !Array.isArray(parsed.entries)) {
            clearCorrupt()
            return []
        }
        return parsed.entries.flatMap((entry) => {
            const normalized = normalizeEntry(entry)
            return normalized ? [normalized] : []
        })
    } catch {
        clearCorrupt()
        return []
    }
}

function clearCorrupt(): void {
    try {
        localStorage.removeItem(SESSION_INDEX_STORAGE_KEY)
    } catch {
        // private mode / quota — nothing more we can do
    }
}

function normalizeEntry(value: unknown): SessionIndexEntry | null {
    if (!value || typeof value !== "object") return null
    const record = value as Record<string, unknown>
    if (
        typeof record.sessionId !== "string"
        || typeof record.cwd !== "string"
        || typeof record.createdAt !== "number"
        || typeof record.lastActiveAt !== "number"
    ) return null
    if (
        record.customCommandFingerprint !== undefined
        && (
            typeof record.customCommandFingerprint !== "string"
            || !/^sha256:[0-9a-f]{64}$/.test(record.customCommandFingerprint)
        )
    ) return null

    const entry: SessionIndexEntry = {
        sessionId: record.sessionId,
        cwd: record.cwd,
        createdAt: record.createdAt,
        lastActiveAt: record.lastActiveAt
    }
    if (typeof record.agentId === "string") entry.agentId = record.agentId
    if (typeof record.customCommandFingerprint === "string") {
        entry.customCommandFingerprint = record.customCommandFingerprint
    }
    if (typeof record.agentTitle === "string") entry.agentTitle = record.agentTitle
    if (typeof record.sessionAlias === "string" || record.sessionAlias === null) {
        entry.sessionAlias = record.sessionAlias
    }
    if (typeof record.derivedTitle === "string") entry.derivedTitle = record.derivedTitle
    return entry
}

function persist(entries: SessionIndexEntry[]): void {
    try {
        const safeEntries = entries.flatMap((entry) => {
            const normalized = normalizeEntry(entry)
            return normalized ? [normalized] : []
        })
        localStorage.setItem(
            SESSION_INDEX_STORAGE_KEY,
            JSON.stringify({
                version: SESSION_INDEX_VERSION,
                entries: safeEntries
            } satisfies SessionIndexEnvelope)
        )
    } catch {
        // private mode / quota — keep the in-memory call as a no-op
    }
}

// 每個 cwd 只保留 lastActiveAt 最新的 MAX_ENTRIES_PER_CWD 筆，其餘視為淘汰。
// 分桶依 normalizeCwd 後的值，讓「/ws」與「/ws/」併入同一桶合計上限。
function prune(entries: SessionIndexEntry[]): SessionIndexEntry[] {
    const byCwd = new Map<string, SessionIndexEntry[]>()
    for (const entry of entries) {
        const key = normalizeCwd(entry.cwd)
        const list = byCwd.get(key) ?? []
        list.push(entry)
        byCwd.set(key, list)
    }
    const kept: SessionIndexEntry[] = []
    for (const list of byCwd.values()) {
        const sorted = [...list].sort((a, b) => b.lastActiveAt - a.lastActiveAt)
        kept.push(...sorted.slice(0, MAX_ENTRIES_PER_CWD))
    }
    return kept
}

export function upsertSessionIndexEntry(rawEntry: SessionIndexEntry): void {
    const normalized = normalizeEntry(rawEntry)
    if (!normalized) return
    const entry = { ...normalized, cwd: normalizeCwd(normalized.cwd) }
    const entries = loadSessionIndex()
    const index = entries.findIndex((candidate) => candidate.sessionId === entry.sessionId)
    const next = index === -1
        ? [...entries, entry]
        : entries.map((candidate, i) => (i === index ? entry : candidate))
    persist(prune(next))
}

export function touchSessionIndexEntry(
    sessionId: string,
    patch: Partial<Omit<SessionIndexEntry, "sessionId" | "cwd" | "createdAt">>
): void {
    const entries = loadSessionIndex()
    const index = entries.findIndex((candidate) => candidate.sessionId === sessionId)
    if (index === -1) return
    const next = entries.map((candidate, i) => {
        if (i !== index) return candidate
        return normalizeEntry({
            ...candidate,
            ...patch,
            sessionId: candidate.sessionId,
            cwd: candidate.cwd,
            createdAt: candidate.createdAt
        }) ?? candidate
    })
    persist(prune(next))
}

export function removeSessionIndexEntry(sessionId: string): void {
    const entries = loadSessionIndex().filter((candidate) => candidate.sessionId !== sessionId)
    persist(entries)
}
