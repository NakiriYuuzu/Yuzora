import { beforeEach, describe, expect, it, vi } from "vitest"

import type { DbQueryResult, DbTable } from "@/lib/types"

vi.mock("@/lib/ipc", () => ({
    dbOpen: vi.fn(),
    dbClose: vi.fn(),
    dbListTables: vi.fn(),
    dbQuery: vi.fn()
}))

import { dbClose, dbListTables, dbOpen, dbQuery } from "@/lib/ipc"
import {
    DB_CONNECTIONS_STORAGE_KEY,
    DB_HISTORY_LIMIT,
    DB_HISTORY_SQL_MAX,
    DB_HISTORY_STORAGE_KEY,
    buildSortedSql,
    buildTableQuery,
    useDbStore
} from "./dbStore"

const mockOpen = vi.mocked(dbOpen)
const mockClose = vi.mocked(dbClose)
const mockList = vi.mocked(dbListTables)
const mockQuery = vi.mocked(dbQuery)

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so history persistence
// runs for real (mirrors sshStore.test.ts).
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

const usersTable: DbTable[] = [{ name: "users", kind: "table" }]
const selectResult: DbQueryResult = {
    kind: "select",
    columns: ["id"],
    rows: [[1], [2]],
    truncated: false
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    useDbStore.getState().reset()
    vi.clearAllMocks()
    let seq = 0
    mockOpen.mockImplementation(async () => ({ connId: `db-${++seq}` }))
    mockClose.mockResolvedValue(undefined)
    mockList.mockResolvedValue(usersTable)
    mockQuery.mockResolvedValue(selectResult)
})

describe("useDbStore", () => {
    it("openConnection registers the connection, activates it, and loads tables", async () => {
        await useDbStore.getState().openConnection("/tmp/app.sqlite")

        const s = useDbStore.getState()
        expect(s.connections).toEqual([
            {
                connId: "db-1",
                kind: "sqlite",
                name: "app.sqlite",
                key: "/tmp/app.sqlite",
                title: "/tmp/app.sqlite"
            }
        ])
        expect(s.activeConnId).toBe("db-1")
        expect(s.tables["db-1"]).toEqual(usersTable)
        expect(mockList).toHaveBeenCalledWith("db-1")
    })

    it("opening a second connection makes it the active one", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")

        const s = useDbStore.getState()
        expect(s.connections.map((c) => c.connId)).toEqual(["db-1", "db-2"])
        expect(s.activeConnId).toBe("db-2")
    })

    it("closeConnection removes it and reassigns active to a remaining connection", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")

        await useDbStore.getState().closeConnection("db-2")

        const s = useDbStore.getState()
        expect(mockClose).toHaveBeenCalledWith("db-2")
        expect(s.connections.map((c) => c.connId)).toEqual(["db-1"])
        expect(s.activeConnId).toBe("db-1")
        expect(s.tables["db-2"]).toBeUndefined()
    })

    it("closing the last connection clears active and drops its query bucket", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()
        expect(useDbStore.getState().queries["db-1"].result).toEqual(selectResult)

        await useDbStore.getState().closeConnection("db-1")

        const s = useDbStore.getState()
        expect(s.activeConnId).toBeNull()
        expect(s.queries["db-1"]).toBeUndefined()
    })

    it("setActiveConnection switches active and loads tables lazily", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        mockList.mockClear()

        useDbStore.getState().setActiveConnection("db-1")

        expect(useDbStore.getState().activeConnId).toBe("db-1")
        // db-1's tables were already loaded on open → no reload.
        expect(mockList).not.toHaveBeenCalled()
    })

    it("runQuery stores the select result and an elapsed time", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")

        await useDbStore.getState().runQuery()

        const q = useDbStore.getState().queries["db-1"]
        expect(mockQuery).toHaveBeenCalledWith("db-1", "SELECT * FROM users")
        expect(q.result).toEqual(selectResult)
        expect(q.error).toBeNull()
        expect(q.running).toBe(false)
        expect(typeof q.elapsedMs).toBe("number")
    })

    it("runQuery reloads tables after an execute (schema may have changed)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockList.mockClear()
        mockQuery.mockResolvedValueOnce({ kind: "execute", affectedRows: 3 })
        useDbStore.getState().setSql("CREATE TABLE t (id)")

        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().queries["db-1"].result).toEqual({ kind: "execute", affectedRows: 3 })
        expect(mockList).toHaveBeenCalledWith("db-1")
    })

    it("runQuery captures a SQL error and clears the previous result", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        mockQuery.mockRejectedValueOnce("no such table: nope")
        useDbStore.getState().setSql("SELECT * FROM nope")
        await useDbStore.getState().runQuery()

        const q = useDbStore.getState().queries["db-1"]
        expect(q.error).toBe("no such table: nope")
        expect(q.result).toBeNull()
        expect(q.running).toBe(false)
    })

    it("runQuery is a no-op without an active connection or with blank sql", async () => {
        // No connection.
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        expect(mockQuery).not.toHaveBeenCalled()

        // Active connection but blank sql.
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("   ")
        await useDbStore.getState().runQuery()
        expect(mockQuery).not.toHaveBeenCalled()
    })

    it("openTableQuery fills a quoted SELECT and runs it", async () => {
        await useDbStore.getState().openConnection("/a.db")

        await useDbStore.getState().openTableQuery("users")

        expect(useDbStore.getState().queries["db-1"].sql).toBe('SELECT * FROM "users" LIMIT 100')
        expect(mockQuery).toHaveBeenCalledWith("db-1", 'SELECT * FROM "users" LIMIT 100')
    })

    it("openTableQuery escapes double quotes in the table name", async () => {
        await useDbStore.getState().openConnection("/a.db")

        await useDbStore.getState().openTableQuery('we"ird')

        expect(useDbStore.getState().queries["db-1"].sql).toBe('SELECT * FROM "we""ird" LIMIT 100')
    })

    it("reset restores the initial state", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")

        useDbStore.getState().reset()

        const s = useDbStore.getState()
        expect(s.connections).toEqual([])
        expect(s.activeConnId).toBeNull()
        expect(s.tables).toEqual({})
        expect(s.queries).toEqual({})
        expect(s.history).toEqual({})
    })

    it("switching connections mid-query keeps a stale result off the newly active connection", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        useDbStore.getState().setActiveConnection("db-1")

        let resolveQuery!: (r: DbQueryResult) => void
        mockQuery.mockImplementationOnce(
            () => new Promise((resolve) => { resolveQuery = resolve })
        )
        useDbStore.getState().setSql("SELECT * FROM users")
        const pending = useDbStore.getState().runQuery()

        // Switch away from db-1 before its query resolves.
        useDbStore.getState().setActiveConnection("db-2")
        expect(useDbStore.getState().queries["db-1"].running).toBe(true)
        expect(useDbStore.getState().queries["db-2"]).toBeUndefined()

        resolveQuery(selectResult)
        await pending

        // The result settles into db-1's own bucket, not onto db-2 (now active).
        expect(useDbStore.getState().queries["db-1"].result).toEqual(selectResult)
        expect(useDbStore.getState().queries["db-1"].running).toBe(false)
        expect(useDbStore.getState().queries["db-2"]).toBeUndefined()

        // Switching back later shows the correctly settled state — running
        // isn't stuck "true" just because the query resolved while elsewhere.
        useDbStore.getState().setActiveConnection("db-1")
        const q = useDbStore.getState().queries["db-1"]
        expect(q.running).toBe(false)
        expect(q.result).toEqual(selectResult)
    })

    it("switching connections mid-query keeps a stale error off the newly active connection's running query", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        useDbStore.getState().setActiveConnection("db-1")

        let rejectQuery!: (e: unknown) => void
        mockQuery.mockImplementationOnce(
            () => new Promise((_resolve, reject) => { rejectQuery = reject })
        )
        useDbStore.getState().setSql("SELECT * FROM broken")
        const pendingA = useDbStore.getState().runQuery()

        // Switch to db-2 and kick off its own (genuinely in-flight) query.
        useDbStore.getState().setActiveConnection("db-2")
        useDbStore.getState().setSql("SELECT * FROM users")
        const pendingB = useDbStore.getState().runQuery()

        // The stale db-1 query now fails.
        rejectQuery("boom")
        await pendingA
        await pendingB

        // db-2's own running query must be unaffected by db-1's stale error.
        expect(useDbStore.getState().queries["db-2"].error).toBeNull()
        expect(useDbStore.getState().queries["db-2"].result).toEqual(selectResult)
        expect(useDbStore.getState().queries["db-2"].running).toBe(false)
        // db-1 keeps its own error, scoped to itself.
        expect(useDbStore.getState().queries["db-1"].error).toBe("boom")
    })
})

describe("useDbStore query history", () => {
    it("records a successful run in the path's history (newest first)", async () => {
        await useDbStore.getState().openConnection("/tmp/app.sqlite")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        const hist = useDbStore.getState().history["/tmp/app.sqlite"]
        expect(hist).toHaveLength(1)
        expect(hist[0]).toMatchObject({ sql: "SELECT 1", ok: true })
        expect(typeof hist[0].ranAt).toBe("number")
        expect(typeof hist[0].elapsedMs).toBe("number")
        expect(hist[0].error).toBeUndefined()
    })

    it("records a failed run with ok:false and the error", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockQuery.mockRejectedValueOnce("no such table: nope")
        useDbStore.getState().setSql("SELECT * FROM nope")
        await useDbStore.getState().runQuery()

        const hist = useDbStore.getState().history["/a.db"]
        expect(hist).toHaveLength(1)
        expect(hist[0]).toMatchObject({
            sql: "SELECT * FROM nope",
            ok: false,
            error: "no such table: nope"
        })
    })

    it("records nothing for a blank query or with no active connection", async () => {
        // No connection.
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        // Active connection but blank sql.
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("   ")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().history).toEqual({})
    })

    it("skips a consecutive duplicate sql", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().history["/a.db"]).toHaveLength(1)
    })

    it("re-records a repeated sql that isn't the most recent (A, B, A)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        useDbStore.getState().setSql("SELECT 2")
        await useDbStore.getState().runQuery()
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().history["/a.db"].map((e) => e.sql)).toEqual([
            "SELECT 1",
            "SELECT 2",
            "SELECT 1"
        ])
    })

    it("caps history at DB_HISTORY_LIMIT, keeping the newest", () => {
        for (let i = 0; i < DB_HISTORY_LIMIT + 10; i++) {
            useDbStore.getState().recordHistory("/a.db", {
                sql: `SELECT ${i}`,
                ranAt: i,
                ok: true,
                elapsedMs: 0
            })
        }
        const hist = useDbStore.getState().history["/a.db"]
        expect(hist).toHaveLength(DB_HISTORY_LIMIT)
        expect(hist[0].sql).toBe(`SELECT ${DB_HISTORY_LIMIT + 9}`)
        expect(hist[hist.length - 1].sql).toBe("SELECT 10")
    })

    it("truncates a stored sql to DB_HISTORY_SQL_MAX characters", () => {
        useDbStore.getState().recordHistory("/a.db", {
            sql: "x".repeat(DB_HISTORY_SQL_MAX + 500),
            ranAt: 1,
            ok: true,
            elapsedMs: 0
        })
        expect(useDbStore.getState().history["/a.db"][0].sql).toHaveLength(DB_HISTORY_SQL_MAX)
    })

    it("keys history by file path — a reopened file keeps it, other files stay separate", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        await useDbStore.getState().closeConnection("db-1")

        // Reopen the same file: new connId, same path → same history bucket.
        await useDbStore.getState().openConnection("/a.db")
        expect(useDbStore.getState().history["/a.db"]).toHaveLength(1)

        // A different file has an independent bucket.
        await useDbStore.getState().openConnection("/b.db")
        useDbStore.getState().setSql("SELECT 2")
        await useDbStore.getState().runQuery()
        expect(useDbStore.getState().history["/b.db"]).toHaveLength(1)
        expect(useDbStore.getState().history["/a.db"]).toHaveLength(1)
    })

    it("persists history to localStorage and reloads it on reset", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeTruthy()

        // reset() reloads from storage, mirroring a fresh app start.
        useDbStore.getState().reset()
        const hist = useDbStore.getState().history["/a.db"]
        expect(hist).toHaveLength(1)
        expect(hist[0].sql).toBe("SELECT 1")
    })
})

describe("useDbStore sort", () => {
    it("runQuery records lastSql for a select and clears any prior sort", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        const q = useDbStore.getState().queries["db-1"]
        expect(q.lastSql).toBe("SELECT * FROM users")
        expect(q.sortBy).toBeNull()
    })

    it("runQuery leaves lastSql null for an execute result (not sortable)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockQuery.mockResolvedValueOnce({ kind: "execute", affectedRows: 1 })
        useDbStore.getState().setSql("UPDATE users SET x = 1")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().queries["db-1"].lastSql).toBeNull()
    })

    it("sortResult wraps the base select as an ordered subquery (asc first, 1-based ordinal)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()
        mockQuery.mockClear()

        await useDbStore.getState().sortResult(1)

        expect(mockQuery).toHaveBeenCalledWith(
            "db-1",
            "SELECT * FROM (\nSELECT * FROM users\n) ORDER BY 2 ASC"
        )
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 1, dir: "asc" })
        // The base stays the original query so a further sort re-wraps it, not the
        // already-wrapped statement.
        expect(useDbStore.getState().queries["db-1"].lastSql).toBe("SELECT * FROM users")
    })

    it("sortResult cycles asc → desc → cleared on the same column", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 0, dir: "asc" })

        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 0, dir: "desc" })

        mockQuery.mockClear()
        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toBeNull()
        // Cleared → re-runs the untouched base query.
        expect(mockQuery).toHaveBeenCalledWith("db-1", "SELECT * FROM users")
    })

    it("sortResult restarts at asc when a different column is clicked", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        await useDbStore.getState().sortResult(0)
        await useDbStore.getState().sortResult(2)

        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 2, dir: "asc" })
    })

    it("sortResult strips a trailing semicolon and line comment before wrapping", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users; -- newest\n")
        await useDbStore.getState().runQuery()
        mockQuery.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQuery).toHaveBeenCalledWith(
            "db-1",
            "SELECT * FROM (\nSELECT * FROM users\n) ORDER BY 1 ASC"
        )
    })

    it("sortResult is a no-op without a select base", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockQuery.mockResolvedValueOnce({ kind: "execute", affectedRows: 1 })
        useDbStore.getState().setSql("UPDATE users SET x = 1")
        await useDbStore.getState().runQuery()
        mockQuery.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQuery).not.toHaveBeenCalled()
    })
})

describe("dbStore dialects", () => {
    it("buildTableQuery uses LIMIT + double quotes for sqlite/postgres", () => {
        expect(buildTableQuery("sqlite", "users")).toBe('SELECT * FROM "users" LIMIT 100')
        expect(buildTableQuery("postgres", 'we"ird')).toBe('SELECT * FROM "we""ird" LIMIT 100')
    })

    it("buildTableQuery uses TOP + brackets for mssql", () => {
        expect(buildTableQuery("mssql", "users")).toBe("SELECT TOP 100 * FROM [users]")
        expect(buildTableQuery("mssql", "we]ird")).toBe("SELECT TOP 100 * FROM [we]]ird]")
    })

    it("buildSortedSql wraps unaliased for sqlite/postgres, aliased for mssql", () => {
        expect(buildSortedSql("sqlite", "SELECT * FROM t", 2, "asc")).toBe(
            "SELECT * FROM (\nSELECT * FROM t\n) ORDER BY 2 ASC"
        )
        expect(buildSortedSql("postgres", "SELECT * FROM t", 1, "desc")).toBe(
            "SELECT * FROM (\nSELECT * FROM t\n) ORDER BY 1 DESC"
        )
        expect(buildSortedSql("mssql", "SELECT * FROM t", 3, "asc")).toBe(
            "SELECT * FROM (\nSELECT * FROM t\n) AS _yz ORDER BY 3 ASC"
        )
    })

    it("sortResult wraps per the active connection's dialect (mssql aliased)", async () => {
        mockOpen.mockImplementationOnce(async () => ({ connId: "mssql-1" }))
        await useDbStore.getState().openConfig({
            kind: "mssql",
            host: "h",
            port: 1433,
            database: "d",
            user: "u",
            password: "secret",
            trustCert: true
        })
        useDbStore.getState().setSql("SELECT * FROM t")
        await useDbStore.getState().runQuery()
        mockQuery.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQuery).toHaveBeenCalledWith(
            "mssql-1",
            "SELECT * FROM (\nSELECT * FROM t\n) AS _yz ORDER BY 1 ASC"
        )
    })
})

describe("dbStore saved connections", () => {
    it("openConfig persists a network descriptor without the password and derives key/name", async () => {
        mockOpen.mockImplementationOnce(async () => ({ connId: "pg-1" }))
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "db.internal",
            port: 5432,
            database: "app",
            user: "admin",
            password: "s3cr3t",
            ssl: true
        })

        const s = useDbStore.getState()
        expect(s.connections[0]).toEqual({
            connId: "pg-1",
            kind: "postgres",
            name: "app@db.internal",
            key: "postgres:db.internal:5432:app",
            title: "admin@db.internal:5432/app"
        })
        expect(s.saved).toEqual([
            {
                id: "postgres:db.internal:5432:app",
                kind: "postgres",
                name: "app@db.internal",
                host: "db.internal",
                port: 5432,
                database: "app",
                user: "admin",
                ssl: true
            }
        ])
        // The password must never reach localStorage.
        const raw = localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY) ?? ""
        expect(raw).not.toContain("s3cr3t")
        expect(raw).toContain("db.internal")
    })

    it("keys network query history by kind:host:port:database", async () => {
        mockOpen.mockImplementationOnce(async () => ({ connId: "pg-1" }))
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "h",
            port: 5432,
            database: "d",
            user: "u",
            password: "p",
            ssl: false
        })
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().history["postgres:h:5432:d"]).toHaveLength(1)
    })

    it("reopening an already-open descriptor focuses it instead of opening twice", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockOpen.mockClear()

        await useDbStore.getState().openConnection("/a.db")

        expect(mockOpen).not.toHaveBeenCalled()
        expect(useDbStore.getState().connections).toHaveLength(1)
    })

    it("removeSaved forgets a descriptor and reset reloads the persisted list", async () => {
        await useDbStore.getState().openConnection("/a.db")
        expect(useDbStore.getState().saved).toHaveLength(1)

        // Persisted descriptor survives a reset (mirrors a fresh app start).
        useDbStore.getState().reset()
        expect(useDbStore.getState().saved).toHaveLength(1)

        useDbStore.getState().removeSaved("/a.db")
        expect(useDbStore.getState().saved).toEqual([])
        useDbStore.getState().reset()
        expect(useDbStore.getState().saved).toEqual([])
    })
})
