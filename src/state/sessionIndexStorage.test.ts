import { beforeEach, describe, expect, it } from "vitest"

import {
    SESSION_INDEX_STORAGE_KEY,
    loadSessionIndex,
    removeSessionIndexEntry,
    touchSessionIndexEntry,
    upsertSessionIndexEntry,
    type SessionIndexEntry
} from "./sessionIndexStorage"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see agentStore.login.test.ts). Install a minimal in-memory
// Storage so persistence is exercised for real.
function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
})

function entry(overrides: Partial<SessionIndexEntry> = {}): SessionIndexEntry {
    return {
        sessionId: "s-1",
        cwd: "/ws",
        agentId: "pi",
        agentTitle: "Fix login bug",
        sessionAlias: null,
        derivedTitle: "derived",
        createdAt: 1,
        lastActiveAt: 1,
        ...overrides
    }
}

describe("sessionIndexStorage", () => {
    it("round-trips an upserted entry", () => {
        expect(loadSessionIndex()).toEqual([])

        upsertSessionIndexEntry(entry())

        expect(loadSessionIndex()).toEqual([entry()])
    })

    it("round-trips a one-way custom command fingerprint without requiring it for v1 entries", () => {
        const customCommandFingerprint = `sha256:${"a".repeat(64)}`
        upsertSessionIndexEntry(entry({ agentId: "custom", customCommandFingerprint }))

        expect(loadSessionIndex()[0]).toMatchObject({ agentId: "custom", customCommandFingerprint })

        localStorage.setItem(SESSION_INDEX_STORAGE_KEY, JSON.stringify({
            version: 1,
            entries: [entry({ agentId: "custom", customCommandFingerprint: undefined })]
        }))
        expect(loadSessionIndex()[0]).toMatchObject({ agentId: "custom" })
        expect(loadSessionIndex()[0].customCommandFingerprint).toBeUndefined()
    })

    it("rejects malformed custom command fingerprints", () => {
        localStorage.setItem(SESSION_INDEX_STORAGE_KEY, JSON.stringify({
            version: 1,
            entries: [entry({ agentId: "custom", customCommandFingerprint: "raw secret command" })]
        }))

        expect(loadSessionIndex()).toEqual([])
    })

    it("upsert replaces an existing entry with the same sessionId instead of duplicating it", () => {
        upsertSessionIndexEntry(entry({ agentTitle: "Old title", lastActiveAt: 1 }))
        upsertSessionIndexEntry(entry({ agentTitle: "New title", lastActiveAt: 2 }))

        const entries = loadSessionIndex()
        expect(entries).toHaveLength(1)
        expect(entries[0]).toMatchObject({ agentTitle: "New title", lastActiveAt: 2 })
    })

    it("touchSessionIndexEntry patches fields without touching sessionId/cwd/createdAt", () => {
        upsertSessionIndexEntry(entry({ createdAt: 100, lastActiveAt: 100 }))

        touchSessionIndexEntry("s-1", { agentTitle: "Renamed", lastActiveAt: 200 })

        const entries = loadSessionIndex()
        expect(entries).toEqual([
            entry({ createdAt: 100, lastActiveAt: 200, agentTitle: "Renamed" })
        ])
    })

    it("touchSessionIndexEntry is a no-op for an unknown sessionId", () => {
        touchSessionIndexEntry("missing", { lastActiveAt: 999 })

        expect(loadSessionIndex()).toEqual([])
    })

    it("removeSessionIndexEntry drops only the matching entry", () => {
        upsertSessionIndexEntry(entry({ sessionId: "s-1" }))
        upsertSessionIndexEntry(entry({ sessionId: "s-2" }))

        removeSessionIndexEntry("s-1")

        const entries = loadSessionIndex()
        expect(entries).toHaveLength(1)
        expect(entries[0].sessionId).toBe("s-2")
    })

    it("stores an envelope with a version field under the expected key", () => {
        upsertSessionIndexEntry(entry())

        const raw = JSON.parse(localStorage.getItem(SESSION_INDEX_STORAGE_KEY)!) as {
            version: number
            entries: SessionIndexEntry[]
        }
        expect(raw.version).toBe(1)
        expect(raw.entries).toHaveLength(1)
    })

    it("whitelists writes so runtime config, usage, cost, pending, and error fields are never persisted", () => {
        upsertSessionIndexEntry({
            ...entry(),
            configOptions: [{ id: "model" }],
            usage: { used: 10, size: 100 },
            cost: { amount: 0.01, currency: "EUR" },
            configRequest: { token: 1 },
            pendingTurn: true,
            configError: "nope",
            error: "also nope"
        } as SessionIndexEntry & Record<string, unknown>)

        const raw = JSON.parse(localStorage.getItem(SESSION_INDEX_STORAGE_KEY)!) as {
            entries: Array<Record<string, unknown>>
        }
        expect(raw.entries[0]).toEqual(entry())
        expect(loadSessionIndex()[0]).toEqual(entry())
    })

    it("whitelists loaded legacy entries and never returns injected runtime fields", () => {
        localStorage.setItem(SESSION_INDEX_STORAGE_KEY, JSON.stringify({
            version: 1,
            entries: [{
                ...entry(),
                configOptions: [{ id: "model" }],
                usage: { used: 10, size: 100, cost: { amount: 0.01, currency: "EUR" } },
                configRequest: { token: 1 },
                pendingTurn: true,
                configError: "nope",
                error: "also nope"
            }]
        }))

        expect(loadSessionIndex()).toEqual([entry()])
    })

    it("clears and returns [] when the version field does not match", () => {
        localStorage.setItem(SESSION_INDEX_STORAGE_KEY, JSON.stringify({ version: 2, entries: [entry()] }))

        expect(loadSessionIndex()).toEqual([])
        expect(localStorage.getItem(SESSION_INDEX_STORAGE_KEY)).toBeNull()
    })

    it("returns [] and clears storage when the persisted JSON is corrupt", () => {
        localStorage.setItem(SESSION_INDEX_STORAGE_KEY, "{not json")

        expect(loadSessionIndex()).toEqual([])
        expect(localStorage.getItem(SESSION_INDEX_STORAGE_KEY)).toBeNull()
    })

    it("keeps only the 20 most-recently-active entries per cwd, evicting the oldest", () => {
        for (let i = 0; i < 25; i++) {
            upsertSessionIndexEntry(entry({ sessionId: `s-${i}`, lastActiveAt: i }))
        }

        const entries = loadSessionIndex()
        expect(entries).toHaveLength(20)
        // The 5 oldest (lastActiveAt 0..4) should have been evicted.
        for (let i = 0; i < 5; i++) {
            expect(entries.some((e) => e.sessionId === `s-${i}`)).toBe(false)
        }
        for (let i = 5; i < 25; i++) {
            expect(entries.some((e) => e.sessionId === `s-${i}`)).toBe(true)
        }
    })

    it("caps each cwd independently — a busy cwd does not evict another cwd's entries", () => {
        for (let i = 0; i < 25; i++) {
            upsertSessionIndexEntry(entry({ sessionId: `busy-${i}`, cwd: "/busy", lastActiveAt: i }))
        }
        upsertSessionIndexEntry(entry({ sessionId: "quiet-1", cwd: "/quiet", lastActiveAt: 0 }))

        const entries = loadSessionIndex()
        expect(entries.filter((e) => e.cwd === "/busy")).toHaveLength(20)
        expect(entries.some((e) => e.sessionId === "quiet-1")).toBe(true)
    })

    it("treats '/ws' and '/ws/' as the same cwd bucket for the 20-entry cap — fixes F12", () => {
        for (let i = 0; i < 25; i++) {
            // 交替寫入帶／不帶結尾斜線的 cwd，模擬同一 workspace 不同呼叫點的路徑格式差異。
            const cwd = i % 2 === 0 ? "/ws" : "/ws/"
            upsertSessionIndexEntry(entry({ sessionId: `s-${i}`, cwd, lastActiveAt: i }))
        }

        const entries = loadSessionIndex()
        expect(entries).toHaveLength(20)
        // 全部歸一化成同一個 cwd 值儲存。
        expect(entries.every((e) => e.cwd === "/ws")).toBe(true)
    })
})
