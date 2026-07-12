import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
    DbColumn,
    DbLiveConnection,
    DbOpenConfig,
    DbProfileDescriptor,
    DbProfileTarget,
    DbQueryCancelResult,
    DbQueryRun,
    DbQueryRunRequest,
    DbQueryResult,
    DbResultPage,
    DbResultSessionOwner,
    DbSaveAndConnectOutcome,
    DbStatementExecution,
    DbTable
} from "@/lib/types"
import { dbObjectRefKey } from "@/lib/databaseSql"

vi.mock("@/lib/ipc", () => ({
    dbListTables: vi.fn(),
    dbTableColumns: vi.fn(),
    dbQueryRun: vi.fn(),
    dbQueryCancel: vi.fn(),
    dbResultPagePrevious: vi.fn(),
    dbResultPageNext: vi.fn(),
    dbResultSessionRelease: vi.fn(),
    dbProfileList: vi.fn(),
    dbProfileImportLegacy: vi.fn(),
    dbProfileCreate: vi.fn(),
    dbProfileUpdate: vi.fn(),
    dbProfileRemoveCredential: vi.fn(),
    dbProfileForget: vi.fn(),
    dbProfileRecover: vi.fn(),
    dbProfileOpen: vi.fn(),
    dbProfileDisconnect: vi.fn()
}))

import {
    dbListTables,
    dbTableColumns,
    dbProfileCreate,
    dbProfileDisconnect,
    dbProfileForget,
    dbProfileImportLegacy,
    dbProfileList,
    dbProfileOpen,
    dbProfileRecover,
    dbProfileRemoveCredential,
    dbProfileUpdate,
    dbQueryCancel,
    dbQueryRun,
    dbResultPageNext,
    dbResultPagePrevious,
    dbResultSessionRelease
} from "@/lib/ipc"
import {
    DB_CONNECTIONS_STORAGE_KEY,
    DB_HISTORY_LIMIT,
    DB_HISTORY_SQL_MAX,
    DB_HISTORY_STORAGE_KEY,
    loadSavedConnections,
    resultPageKey,
    useDbStore
} from "./dbStore"

const mockList = vi.mocked(dbListTables)
const mockColumns = vi.mocked(dbTableColumns)
const mockQueryRun = vi.mocked(dbQueryRun)
const mockQueryCancel = vi.mocked(dbQueryCancel)
const mockResultPagePrevious = vi.mocked(dbResultPagePrevious)
const mockResultPageNext = vi.mocked(dbResultPageNext)
const mockResultSessionRelease = vi.mocked(dbResultSessionRelease)
const mockProfileList = vi.mocked(dbProfileList)
const mockProfileImportLegacy = vi.mocked(dbProfileImportLegacy)
const mockProfileCreate = vi.mocked(dbProfileCreate)
const mockProfileUpdate = vi.mocked(dbProfileUpdate)
const mockProfileRemoveCredential = vi.mocked(dbProfileRemoveCredential)
const mockProfileForget = vi.mocked(dbProfileForget)
const mockProfileRecover = vi.mocked(dbProfileRecover)
const mockProfileOpen = vi.mocked(dbProfileOpen)
const mockProfileDisconnect = vi.mocked(dbProfileDisconnect)

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so legacy migration and
// the no-new-persistence assertions run against a real Storage-shaped seam.
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

const usersTable: DbTable[] = [
    { catalog: "main", schema: "main", name: "users", kind: "table" }
]
const selectResult: Extract<DbQueryResult, { kind: "select" }> = {
    kind: "select",
    columns: ["id"],
    rows: [
        [{ kind: "integer", value: "1" }],
        [{ kind: "integer", value: "2" }]
    ],
    truncated: false,
    affectedRows: null,
    effectOutcome: "unknown"
}

function queryRunFromResult(request: DbQueryRunRequest, result: DbQueryResult): DbQueryRun {
    const firstStatementExecutionId = `${request.queryRunId}:statement:0` as DbQueryRun["statements"][number]["statementExecutionId"]
    const resultOwner = {
        descriptorId: request.descriptorId,
        connectionId: request.connectionId,
        connectionGeneration: request.connectionGeneration,
        queryRunId: request.queryRunId,
        statementExecutionId: firstStatementExecutionId,
        resultSessionId: `${request.queryRunId}:result:0` as never
    }
    const firstResult = result.kind === "select"
        ? {
              kind: "rows" as const,
              resultSession: {
                  owner: resultOwner,
                  columns: result.columns,
                  initialPage: {
                      owner: resultOwner,
                      pageIndex: 0,
                      columns: result.columns,
                      rows: result.rows,
                      hasPrevious: false,
                      hasNext: result.truncated,
                      effectOutcome: result.effectOutcome,
                      lifecycle: result.truncated ? "streaming" : "complete",
                      resultLimitReached: false
                  }
              },
              affectedRows: result.affectedRows
          }
        : { kind: "execute" as const, affectedRows: result.affectedRows }
    const statements = request.statements.map((statement, statementIndex) => ({
        statementExecutionId: `${request.queryRunId}:statement:${statementIndex}` as never,
        statementIndex,
        sql: statement.sql,
        effectOutcome: statementIndex === 0 ? result.effectOutcome : "none" as const,
        result: statementIndex === 0 ? firstResult : { kind: "skipped" as const }
    })) as unknown as DbQueryRun["statements"]
    return {
        descriptorId: request.descriptorId,
        connectionId: request.connectionId,
        connectionGeneration: request.connectionGeneration,
        queryRunId: request.queryRunId,
        statements,
        transactionMayBeOpen: false,
        connectionTerminated: false
    }
}

function mockRunResultOnce(result: DbQueryResult): void {
    mockQueryRun.mockImplementationOnce(async (request) => queryRunFromResult(request, result))
}

function rowExecution(
    request: DbQueryRunRequest,
    statementIndex: number,
    result: Extract<DbQueryResult, { kind: "select" }> = selectResult
): DbStatementExecution {
    const statementExecutionId = `${request.queryRunId}:statement:${statementIndex}` as never
    const owner = {
        descriptorId: request.descriptorId,
        connectionId: request.connectionId,
        connectionGeneration: request.connectionGeneration,
        queryRunId: request.queryRunId,
        statementExecutionId,
        resultSessionId: `${request.queryRunId}:result:${statementIndex}` as never
    }
    return {
        statementExecutionId,
        statementIndex,
        sql: request.statements[statementIndex].sql,
        effectOutcome: result.effectOutcome,
        result: {
            kind: "rows",
            resultSession: {
                owner,
                columns: result.columns,
                initialPage: {
                    owner,
                    pageIndex: 0,
                    columns: result.columns,
                    rows: result.rows,
                    hasPrevious: false,
                    hasNext: result.truncated,
                    effectOutcome: result.effectOutcome,
                    lifecycle: result.truncated ? "streaming" : "complete",
                    resultLimitReached: false
                }
            },
            affectedRows: result.affectedRows
        }
    }
}

function executeExecution(
    request: DbQueryRunRequest,
    statementIndex: number,
    effectOutcome: DbStatementExecution["effectOutcome"] = "unknown"
): DbStatementExecution {
    return {
        statementExecutionId: `${request.queryRunId}:statement:${statementIndex}` as never,
        statementIndex,
        sql: request.statements[statementIndex].sql,
        effectOutcome,
        result: { kind: "execute", affectedRows: "1" }
    }
}

function queryRunWithStatements(
    request: DbQueryRunRequest,
    statements: DbQueryRun["statements"],
    overrides: Partial<Pick<DbQueryRun, "transactionMayBeOpen" | "connectionTerminated">> = {}
): DbQueryRun {
    return {
        descriptorId: request.descriptorId,
        connectionId: request.connectionId,
        connectionGeneration: request.connectionGeneration,
        queryRunId: request.queryRunId,
        statements,
        transactionMayBeOpen: false,
        connectionTerminated: false,
        ...overrides
    }
}

function wirePage(
    owner: DbResultSessionOwner,
    overrides: Partial<Omit<DbResultPage, "owner">> = {}
): DbResultPage {
    return {
        owner,
        pageIndex: 0,
        columns: ["id"],
        rows: [[{ kind: "integer", value: "1" }]],
        hasPrevious: false,
        hasNext: false,
        effectOutcome: "none",
        lifecycle: "complete",
        resultLimitReached: false,
        ...overrides
    }
}

function resultOwnerOf(statement: DbStatementExecution): DbResultSessionOwner {
    if (statement.result.kind === "rows" && statement.result.resultSession) {
        return statement.result.resultSession.owner
    }
    if (statement.result.kind === "resultLimitReached") {
        return statement.result.resultSession.owner
    }
    throw new Error("expected statement result session")
}

function storedResultPage(descriptorId: string, owner: DbResultSessionOwner) {
    const state = useDbStore.getState().queryBuckets[descriptorId]
        ?.runGroup?.resultPages[resultPageKey(owner)]
    if (!state) throw new Error("expected stored result page")
    return state
}

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (reason?: unknown) => void
    const promise = new Promise<T>((res, rej) => {
        resolve = res
        reject = rej
    })
    return { promise, resolve, reject }
}

function live(
    descriptorId: string,
    connectionId: string,
    connectionGeneration: string
): DbLiveConnection {
    return {
        descriptorId: descriptorId as DbLiveConnection["descriptorId"],
        connectionId: connectionId as DbLiveConnection["connectionId"],
        connectionGeneration: connectionGeneration as DbLiveConnection["connectionGeneration"],
        engine: "sqlite"
    }
}

function snapshotProfiles(): DbProfileDescriptor[] {
    return useDbStore.getState().saved.flatMap<DbProfileDescriptor>((saved): DbProfileDescriptor[] => {
        if (saved.kind === "sqlite" && saved.path) {
            return [{
                descriptorId: saved.id as DbProfileDescriptor["descriptorId"],
                configGeneration: saved.configGeneration ?? 1,
                name: saved.name,
                target: { kind: "sqlite" as const, path: saved.path },
                credentialState: saved.credentialState ?? "notRequired" as const
            }]
        }
        if (saved.kind !== "sqlite" && saved.host && saved.port && saved.database && saved.user) {
            const target: DbProfileTarget = saved.kind === "postgres"
                ? {
                      kind: "postgres",
                      host: saved.host,
                      port: saved.port,
                      database: saved.database,
                      user: saved.user,
                      ssl: saved.ssl ?? false,
                      trustCert: saved.trustCert ?? false
                  }
                : {
                      kind: "mssql",
                      host: saved.host,
                      port: saved.port,
                      database: saved.database,
                      user: saved.user,
                      trustCert: saved.trustCert ?? false
                  }
            return [{
                descriptorId: saved.id as DbProfileDescriptor["descriptorId"],
                configGeneration: saved.configGeneration ?? 1,
                name: saved.name,
                target,
                credentialState: saved.credentialState ?? "stored" as const
            }]
        }
        return []
    })
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    useDbStore.getState().reset()
    vi.clearAllMocks()
    let connectionSeq = 0
    mockList.mockResolvedValue(usersTable)
    mockColumns.mockResolvedValue([
        { name: "id", type: "INTEGER", notnull: true, pk: true }
    ])
    mockQueryRun.mockImplementation(async (request) => queryRunFromResult(request, selectResult))
    mockQueryCancel.mockResolvedValue({ outcome: "cancelled" })
    mockResultPagePrevious.mockImplementation(async (owner) => wirePage(owner))
    mockResultPageNext.mockImplementation(async (owner) => wirePage(owner))
    mockResultSessionRelease.mockImplementation(async (owner) => wirePage(owner, {
        lifecycle: "released"
    }))
    let profileSeq = 0
    mockProfileList.mockImplementation(async () => ({ profiles: snapshotProfiles(), recovery: [] }))
    mockProfileImportLegacy.mockImplementation(async (request) => ({
        profiles: request.profiles,
        recovery: []
    }))
    mockProfileCreate.mockImplementation(async (request) => {
        const descriptorId = `dbc-test-${++profileSeq}` as DbProfileDescriptor["descriptorId"]
        const profile: DbProfileDescriptor = {
            descriptorId,
            configGeneration: 1,
            name: request.name,
            target: request.target,
            credentialState: request.target.kind === "sqlite" ? "notRequired" : "stored"
        }
        const connectionId = `db-${++connectionSeq}`
        return {
            outcome: "connected",
            profile,
            connection: {
                descriptorId,
                connectionId: connectionId as never,
                connectionGeneration: `generation-${profileSeq}` as never,
                engine: request.target.kind
            }
        }
    })
    mockProfileUpdate.mockImplementation(async (request) => ({
        descriptorId: request.descriptorId,
        configGeneration:
            (useDbStore.getState().saved.find((profile) => profile.id === request.descriptorId)
                ?.configGeneration ?? 1) + 1,
        name: request.name,
        target: request.target,
        credentialState: request.target.kind === "sqlite" ? "notRequired" : "stored"
    }))
    mockProfileForget.mockImplementation(async (descriptorId) => ({
        profiles: snapshotProfiles().filter((profile) => profile.descriptorId !== descriptorId),
        recovery: []
    }))
    mockProfileRemoveCredential.mockImplementation(async (descriptorId) => ({
        profiles: snapshotProfiles().map((profile) => profile.descriptorId === descriptorId
            ? { ...profile, credentialState: "required" as const }
            : profile),
        recovery: []
    }))
    mockProfileRecover.mockImplementation(async () => ({ profiles: snapshotProfiles(), recovery: [] }))
    mockProfileOpen.mockImplementation(async (descriptorId) => {
        const profile = snapshotProfiles().find((item) => item.descriptorId === descriptorId)
        if (!profile) throw { code: "profileNotFound", message: "missing" }
        const connectionId = `db-${++connectionSeq}`
        return {
            descriptorId,
            connectionId: connectionId as never,
            connectionGeneration: `open-generation-${++profileSeq}` as never,
            engine: profile.target.kind
        }
    })
    mockProfileDisconnect.mockResolvedValue(undefined)
})

describe("useDbStore", () => {
    it("openConnection registers the connection, activates it, and loads tables", async () => {
        await useDbStore.getState().openConnection("/tmp/app.sqlite")

        const s = useDbStore.getState()
        expect(s.connections).toEqual([
            {
                connId: "db-1",
                connectionGeneration: expect.any(String),
                kind: "sqlite",
                name: "app.sqlite",
                descriptorId: expect.any(String),
                targetKey: "/tmp/app.sqlite",
                title: "/tmp/app.sqlite"
            }
        ])
        expect(s.activeConnId).toBe("db-1")
        expect(s.tables["db-1"]).toEqual(usersTable)
        expect(mockList).toHaveBeenCalledWith({
            descriptorId: s.activeDescriptorId,
            connectionId: "db-1",
            connectionGeneration: expect.any(String)
        })
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
        expect(mockProfileDisconnect).toHaveBeenCalledWith(expect.objectContaining({
            connectionId: "db-2",
            connectionGeneration: expect.any(String)
        }))
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
        expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
            descriptorId: useDbStore.getState().activeDescriptorId,
            connectionId: "db-1",
            connectionGeneration: expect.any(String),
            queryRunId: expect.any(String),
            statements: [{ sql: "SELECT * FROM users", transactionBoundary: "none" }]
        }))
        expect(q.result).toEqual(selectResult)
        expect(q.error).toBeNull()
        expect(q.running).toBe(false)
        expect(typeof q.elapsedMs).toBe("number")
    })

    it("runQuery reloads tables after an execute (schema may have changed)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockList.mockClear()
        mockRunResultOnce({ kind: "execute", affectedRows: "3", effectOutcome: "unknown" })
        useDbStore.getState().setSql("CREATE TABLE t (id)")

        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().queries["db-1"].result).toEqual({
            kind: "execute",
            affectedRows: "3",
            effectOutcome: "unknown"
        })
        expect(mockList).toHaveBeenCalledWith(expect.objectContaining({ connectionId: "db-1" }))
    })

    it("runQuery captures a SQL error and clears the previous result", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        mockQueryRun.mockRejectedValueOnce("no such table: nope")
        useDbStore.getState().setSql("SELECT * FROM nope")
        await useDbStore.getState().runQuery()

        const q = useDbStore.getState().queries["db-1"]
        expect(q.error).toEqual({
            code: "queryFailed",
            databaseError: null,
            executedSql: "SELECT * FROM nope"
        })
        expect(q.result).toBeNull()
        expect(q.running).toBe(false)
    })

    it("keeps exact executed SQL with a typed engine error without persisting it", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const databaseError = {
            engine: "postgres" as const,
            message: "syntax error at or near FROM",
            code: "42601",
            position: { offset: 12, line: null, column: null },
            detail: "select list is incomplete",
            hint: "add an expression",
            retryability: "notRetryable" as const
        }
        mockQueryRun.mockRejectedValueOnce({
            code: "queryFailed",
            message: "database query failed",
            error: databaseError
        })
        const editorSql = "  SELECT 雪, FROM data  \n"
        const executedUnit = "SELECT 雪, FROM data"
        useDbStore.getState().setSql(editorSql)

        await useDbStore.getState().runQuery()

        expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
            statements: [{ sql: executedUnit, transactionBoundary: "none" }]
        }))
        expect(useDbStore.getState().queries["db-1"].error).toEqual({
            code: "queryFailed",
            databaseError,
            executedSql: executedUnit
        })
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
        expect(JSON.stringify(localStorage)).not.toContain("SELECT 雪")
        expect(JSON.stringify(localStorage)).not.toContain("syntax error")
    })

    it("runQuery is a no-op without an active connection or with blank sql", async () => {
        // No connection.
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        expect(mockQueryRun).not.toHaveBeenCalled()

        // Active connection but blank sql.
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("   ")
        await useDbStore.getState().runQuery()
        expect(mockQueryRun).not.toHaveBeenCalled()
    })

    it("openTableQuery fills a quoted SELECT and runs it", async () => {
        await useDbStore.getState().openConnection("/a.db")

        await useDbStore.getState().openTableQuery(usersTable[0])

        expect(useDbStore.getState().queries["db-1"].sql).toBe(
            'SELECT * FROM "main"."users" LIMIT 100'
        )
        expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
            connectionId: "db-1",
            statements: [{
                sql: 'SELECT * FROM "main"."users" LIMIT 100',
                transactionBoundary: "none"
            }]
        }))
    })

    it("openTableQuery escapes double quotes in the table name", async () => {
        await useDbStore.getState().openConnection("/a.db")

        await useDbStore.getState().openTableQuery({
            catalog: "main",
            schema: 'odd"schema',
            name: 'we"ird',
            kind: "table"
        })

        expect(useDbStore.getState().queries["db-1"].sql).toBe(
            'SELECT * FROM "odd""schema"."we""ird" LIMIT 100'
        )
    })

    it("reset restores the initial state", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")

        useDbStore.getState().reset()

        const s = useDbStore.getState()
        expect(s.connections).toEqual([])
        expect(s.activeConnId).toBeNull()
        expect(s.sessions).toEqual({})
        expect(s.tables).toEqual({})
        expect(s.queries).toEqual({})
        expect(s.historyBuckets).toEqual({})
    })

    it("switching connections mid-query keeps a stale result off the newly active connection", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        useDbStore.getState().setActiveConnection("db-1")

        let queryRequest!: DbQueryRunRequest
        let resolveQuery!: (run: DbQueryRun) => void
        mockQueryRun.mockImplementationOnce((request) => {
            queryRequest = request
            return new Promise((resolve) => { resolveQuery = resolve })
        })
        useDbStore.getState().setSql("SELECT * FROM users")
        const pending = useDbStore.getState().runQuery()

        // Switch away from db-1 before its query resolves.
        useDbStore.getState().setActiveConnection("db-2")
        expect(useDbStore.getState().queries["db-1"].running).toBe(true)
        expect(useDbStore.getState().queries["db-2"]).toBeUndefined()

        resolveQuery(queryRunFromResult(queryRequest, selectResult))
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
        mockQueryRun.mockImplementationOnce(
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
        expect(useDbStore.getState().queries["db-1"].error).toMatchObject({
            code: "queryFailed",
            executedSql: "SELECT * FROM broken"
        })
    })
})

describe("dbStore P7 per-statement result pages", () => {
    const pagedResult = (values: string[]): Extract<DbQueryResult, { kind: "select" }> => ({
        kind: "select",
        columns: ["value"],
        rows: values.map((value) => [{ kind: "integer", value }]),
        truncated: true,
        affectedRows: null,
        effectOutcome: "none"
    })

    it("isolates page, loading, and sort state across two tabs and profile switches", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT first_value();\nSELECT second_value();")
        mockQueryRun.mockImplementationOnce(async (request) => queryRunWithStatements(request, [
            rowExecution(request, 0, pagedResult(["2", "1"])),
            rowExecution(request, 1, pagedResult(["20", "10"]))
        ]))

        await useDbStore.getState().runQuery({ kind: "all" })

        const run = useDbStore.getState().queryBuckets[descriptorId].runGroup!.run!
        const firstOwner = resultOwnerOf(run.statements[0])
        const secondOwner = resultOwnerOf(run.statements[1])
        await useDbStore.getState().sortResult(0, firstOwner)
        useDbStore.getState().selectStatementTab(run.statements[1].statementExecutionId)
        await useDbStore.getState().sortResult(0, secondOwner)

        expect(storedResultPage(descriptorId, firstOwner)).toMatchObject({
            sort: { columnIndex: 0, dir: "asc" },
            page: { pageIndex: 0, rows: [
                [{ kind: "integer", value: "1" }],
                [{ kind: "integer", value: "2" }]
            ] }
        })
        expect(storedResultPage(descriptorId, secondOwner)).toMatchObject({
            sort: { columnIndex: 0, dir: "asc" },
            page: { pageIndex: 0, rows: [
                [{ kind: "integer", value: "10" }],
                [{ kind: "integer", value: "20" }]
            ] }
        })
        const firstInitial = run.statements[0].result
        if (firstInitial.kind !== "rows") throw new Error("expected rows")
        expect(firstInitial.resultSession?.initialPage.rows).toEqual([
            [{ kind: "integer", value: "2" }],
            [{ kind: "integer", value: "1" }]
        ])

        const firstNext = deferred<DbResultPage>()
        const secondNext = deferred<DbResultPage>()
        mockResultPageNext.mockImplementation((owner) =>
            owner.resultSessionId === firstOwner.resultSessionId
                ? firstNext.promise
                : secondNext.promise
        )
        const firstPending = useDbStore.getState().nextResultPage(firstOwner)
        const secondPending = useDbStore.getState().nextResultPage(secondOwner)
        expect(storedResultPage(descriptorId, firstOwner).loading).toBe(true)
        expect(storedResultPage(descriptorId, secondOwner).loading).toBe(true)

        secondNext.resolve(wirePage(secondOwner, {
            pageIndex: 1,
            columns: ["value"],
            rows: [
                [{ kind: "integer", value: "40" }],
                [{ kind: "integer", value: "30" }]
            ],
            hasPrevious: true,
            lifecycle: "complete"
        }))
        await secondPending
        expect(storedResultPage(descriptorId, firstOwner)).toMatchObject({
            loading: true,
            page: { pageIndex: 0 }
        })
        expect(storedResultPage(descriptorId, secondOwner)).toMatchObject({
            loading: false,
            sort: { columnIndex: 0, dir: "asc" },
            page: { pageIndex: 1, rows: [
                [{ kind: "integer", value: "30" }],
                [{ kind: "integer", value: "40" }]
            ] }
        })

        firstNext.resolve(wirePage(firstOwner, {
            pageIndex: 1,
            columns: ["value"],
            rows: [
                [{ kind: "integer", value: "4" }],
                [{ kind: "integer", value: "3" }]
            ],
            hasPrevious: true,
            lifecycle: "complete"
        }))
        await firstPending
        expect(storedResultPage(descriptorId, firstOwner).page.rows).toEqual([
            [{ kind: "integer", value: "3" }],
            [{ kind: "integer", value: "4" }]
        ])

        await useDbStore.getState().openConnection("/b.db")
        useDbStore.getState().setActiveDescriptor(descriptorId)
        expect(storedResultPage(descriptorId, firstOwner)).toMatchObject({
            sort: { columnIndex: 0, dir: "asc" },
            page: { pageIndex: 1 }
        })
        expect(storedResultPage(descriptorId, secondOwner)).toMatchObject({
            sort: { columnIndex: 0, dir: "asc" },
            page: { pageIndex: 1 }
        })
    })

    it("uses exact owners for Next, cached Previous, and final-page Release", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT paged_value();")
        mockRunResultOnce(pagedResult(["2", "1"]))
        await useDbStore.getState().runQuery()
        const statement = useDbStore.getState().queryBuckets[descriptorId].runGroup!.run!.statements[0]
        const owner = resultOwnerOf(statement)
        await useDbStore.getState().sortResult(0, owner)

        mockResultPageNext.mockResolvedValue(wirePage(owner, {
            pageIndex: 1,
            columns: ["value"],
            rows: [
                [{ kind: "integer", value: "4" }],
                [{ kind: "integer", value: "3" }]
            ],
            hasPrevious: true,
            hasNext: true,
            lifecycle: "streaming",
            effectOutcome: "transactionPending"
        }))
        await useDbStore.getState().nextResultPage(owner)
        expect(mockResultPageNext).toHaveBeenCalledWith(owner)
        expect(storedResultPage(descriptorId, owner).page.rows).toEqual([
            [{ kind: "integer", value: "3" }],
            [{ kind: "integer", value: "4" }]
        ])

        mockResultPagePrevious.mockResolvedValueOnce(wirePage(owner, {
            columns: ["value"],
            rows: [
                [{ kind: "integer", value: "2" }],
                [{ kind: "integer", value: "1" }]
            ],
            hasNext: true,
            lifecycle: "streaming"
        }))
        await useDbStore.getState().previousResultPage(owner)
        expect(mockResultPagePrevious).toHaveBeenCalledWith(owner)
        expect(storedResultPage(descriptorId, owner).page.pageIndex).toBe(0)

        await useDbStore.getState().nextResultPage(owner)
        mockResultSessionRelease.mockResolvedValueOnce(wirePage(owner, {
            pageIndex: 1,
            columns: ["value"],
            rows: [
                [{ kind: "integer", value: "6" }],
                [{ kind: "integer", value: "5" }]
            ],
            hasPrevious: true,
            hasNext: true,
            lifecycle: "complete",
            effectOutcome: "committed"
        }))
        await useDbStore.getState().releaseResultSession(owner)

        expect(mockResultSessionRelease).toHaveBeenCalledWith(owner)
        expect(storedResultPage(descriptorId, owner)).toMatchObject({
            loading: false,
            pageError: null,
            released: true,
            sort: { columnIndex: 0, dir: "asc" },
            page: {
                pageIndex: 1,
                rows: [
                    [{ kind: "integer", value: "5" }],
                    [{ kind: "integer", value: "6" }]
                ],
                effectOutcome: "committed",
                hasNext: false,
                lifecycle: "released"
            }
        })
        const nextCalls = mockResultPageNext.mock.calls.length
        await useDbStore.getState().nextResultPage(owner)
        expect(mockResultPageNext).toHaveBeenCalledTimes(nextCalls)
    })

    it("navigates cached complete and result-limit pages when hasNext is true", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT materialized();")
        mockQueryRun.mockImplementationOnce(async (request) => {
            const statement = rowExecution(request, 0, pagedResult(["1"]))
            if (statement.result.kind !== "rows" || !statement.result.resultSession) {
                throw new Error("expected rows result")
            }
            statement.result.resultSession.initialPage = {
                ...statement.result.resultSession.initialPage,
                lifecycle: "complete",
                hasNext: true
            }
            return queryRunWithStatements(request, [statement])
        })
        await useDbStore.getState().runQuery()
        let statement = useDbStore.getState().queryBuckets[descriptorId].runGroup!.run!.statements[0]
        let owner = resultOwnerOf(statement)
        mockResultPageNext.mockResolvedValueOnce(wirePage(owner, {
            pageIndex: 1,
            hasPrevious: true,
            lifecycle: "complete"
        }))

        await useDbStore.getState().nextResultPage(owner)
        expect(mockResultPageNext).toHaveBeenCalledWith(owner)
        expect(storedResultPage(descriptorId, owner).page.pageIndex).toBe(1)

        useDbStore.getState().setSql("SELECT limited_materialized();")
        mockQueryRun.mockImplementationOnce(async (request) => {
            const rows = rowExecution(request, 0, pagedResult(["500"]))
            if (rows.result.kind !== "rows" || !rows.result.resultSession) {
                throw new Error("expected rows result")
            }
            return queryRunWithStatements(request, [{
                ...rows,
                result: {
                    kind: "resultLimitReached" as const,
                    affectedRows: null,
                    resultSession: {
                        ...rows.result.resultSession,
                        initialPage: {
                            ...rows.result.resultSession.initialPage,
                            lifecycle: "complete" as const,
                            hasNext: true,
                            resultLimitReached: true
                        }
                    }
                }
            }])
        })
        await useDbStore.getState().runQuery()
        statement = useDbStore.getState().queryBuckets[descriptorId].runGroup!.run!.statements[0]
        owner = resultOwnerOf(statement)
        mockResultPageNext.mockResolvedValueOnce(wirePage(owner, {
            pageIndex: 1,
            hasPrevious: true,
            lifecycle: "complete",
            resultLimitReached: true
        }))

        await useDbStore.getState().nextResultPage(owner)
        expect(mockResultPageNext).toHaveBeenLastCalledWith(owner)
        expect(storedResultPage(descriptorId, owner).page.pageIndex).toBe(1)
    })

    it.each(["released", "cancelled", "error"] as const)(
        "does not navigate a %s result even when hasNext is true",
        async (lifecycle) => {
            await useDbStore.getState().openConnection("/a.db")
            const descriptorId = useDbStore.getState().activeDescriptorId!
            useDbStore.getState().setSql(`SELECT ${lifecycle}_result();`)
            mockQueryRun.mockImplementationOnce(async (request) => {
                const statement = rowExecution(request, 0, pagedResult(["1"]))
                if (statement.result.kind !== "rows" || !statement.result.resultSession) {
                    throw new Error("expected rows result")
                }
                statement.result.resultSession.initialPage = {
                    ...statement.result.resultSession.initialPage,
                    lifecycle,
                    hasNext: true
                }
                return queryRunWithStatements(request, [statement])
            })
            await useDbStore.getState().runQuery()
            const owner = resultOwnerOf(
                useDbStore.getState().queryBuckets[descriptorId].runGroup!.run!.statements[0]
            )
            mockResultPageNext.mockClear()

            await useDbStore.getState().nextResultPage(owner)

            expect(mockResultPageNext).not.toHaveBeenCalled()
            expect(storedResultPage(descriptorId, owner).page.pageIndex).toBe(0)
        }
    )

    it("cancels a settled run only through its active exact streaming page", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT active_stream();\nSELECT other_cache();")
        mockQueryRun.mockImplementationOnce(async (request) => queryRunWithStatements(request, [
            rowExecution(request, 0, pagedResult(["2", "1"])),
            rowExecution(request, 1, pagedResult(["20", "10"]))
        ]))
        await useDbStore.getState().runQuery({ kind: "all" })
        const group = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        const activeOwner = resultOwnerOf(group.run!.statements[0])
        const otherOwner = resultOwnerOf(group.run!.statements[1])
        const otherBefore = storedResultPage(descriptorId, otherOwner)
        expect(group.status).toBe("settled")

        await useDbStore.getState().cancelQuery()

        expect(mockQueryCancel).toHaveBeenCalledWith(group.owner)
        expect(storedResultPage(descriptorId, activeOwner)).toMatchObject({
            loading: false,
            page: {
                lifecycle: "cancelled",
                hasNext: false,
                rows: [
                    [{ kind: "integer", value: "2" }],
                    [{ kind: "integer", value: "1" }]
                ]
            }
        })
        expect(storedResultPage(descriptorId, otherOwner)).toEqual(otherBefore)
        mockResultPageNext.mockClear()
        await useDbStore.getState().nextResultPage(activeOwner)
        expect(mockResultPageNext).not.toHaveBeenCalled()
    })

    it.each(["complete", "released", "cancelled", "error"] as const)(
        "does not cancel a settled %s result page",
        async (lifecycle) => {
            await useDbStore.getState().openConnection("/a.db")
            const descriptorId = useDbStore.getState().activeDescriptorId!
            useDbStore.getState().setSql(`SELECT ${lifecycle}_cache();`)
            mockQueryRun.mockImplementationOnce(async (request) => {
                const statement = rowExecution(request, 0, pagedResult(["1"]))
                if (statement.result.kind !== "rows" || !statement.result.resultSession) {
                    throw new Error("expected rows result")
                }
                statement.result.resultSession.initialPage = {
                    ...statement.result.resultSession.initialPage,
                    lifecycle
                }
                return queryRunWithStatements(request, [statement])
            })
            await useDbStore.getState().runQuery()
            expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.status).toBe("settled")
            mockQueryCancel.mockClear()

            await useDbStore.getState().cancelQuery()

            expect(mockQueryCancel).not.toHaveBeenCalled()
        }
    )

    it("drops a late settled-stream cancel response after a newer owner replaces the page", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT old_stream();")
        mockRunResultOnce(pagedResult(["1"]))
        await useDbStore.getState().runQuery()
        const oldGroup = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        const cancel = deferred<DbQueryCancelResult>()
        mockQueryCancel.mockReturnValueOnce(cancel.promise)
        const pendingCancel = useDbStore.getState().cancelQuery()

        useDbStore.getState().setSql("SELECT new_cache();")
        await useDbStore.getState().runQuery()
        const newGroup = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        expect(newGroup.owner.queryRunId).not.toBe(oldGroup.owner.queryRunId)
        cancel.resolve({ outcome: "cancelled" })
        await pendingCancel

        const current = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        expect(current.owner).toEqual(newGroup.owner)
        const currentOwner = resultOwnerOf(current.run!.statements[0])
        expect(storedResultPage(descriptorId, currentOwner).page).toMatchObject({
            lifecycle: "complete",
            rows: selectResult.rows
        })
        expect(current.cancelOutcome).toBeNull()
    })
})

describe("useDbStore session-only query history", () => {
    it("records a successful run in the active descriptor's history (newest first)", async () => {
        await useDbStore.getState().openConnection("/tmp/app.sqlite")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        const hist = useDbStore.getState().historyBuckets[descriptorId]
        expect(hist).toHaveLength(1)
        expect(hist[0]).toMatchObject({ sql: "SELECT 1", ok: true })
        expect(typeof hist[0].ranAt).toBe("number")
        expect(typeof hist[0].elapsedMs).toBe("number")
        expect(hist[0].error).toBeUndefined()
    })

    it("records a failed run with ok:false and the error", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        mockQueryRun.mockRejectedValueOnce("no such table: nope")
        useDbStore.getState().setSql("SELECT * FROM nope")
        await useDbStore.getState().runQuery()

        const hist = useDbStore.getState().historyBuckets[descriptorId]
        expect(hist).toHaveLength(1)
        expect(hist[0]).toMatchObject({
            sql: "SELECT * FROM nope",
            ok: false,
            error: "queryFailed"
        })
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
    })

    it("records nothing for a blank query or with no active connection", async () => {
        // No connection.
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        // Active connection but blank sql.
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("   ")
        await useDbStore.getState().runQuery()

        const descriptorId = useDbStore.getState().activeDescriptorId!
        expect(useDbStore.getState().historyBuckets[descriptorId]).toEqual([])
    })

    it("skips a consecutive duplicate sql", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().historyBuckets[descriptorId]).toHaveLength(1)
    })

    it("re-records a repeated sql that isn't the most recent (A, B, A)", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        useDbStore.getState().setSql("SELECT 2")
        await useDbStore.getState().runQuery()
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().historyBuckets[descriptorId].map((e) => e.sql)).toEqual([
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
        const hist = useDbStore.getState().historyBuckets["/a.db"]
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
        expect(useDbStore.getState().historyBuckets["/a.db"][0].sql).toHaveLength(DB_HISTORY_SQL_MAX)
    })

    it("keeps history across reconnect of the same descriptor while other descriptors stay separate", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorA = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        await useDbStore.getState().closeConnection("db-1")

        await useDbStore.getState().openOrReconnectSavedConnection(descriptorA)
        expect(useDbStore.getState().historyBuckets[descriptorA]).toHaveLength(1)

        await useDbStore.getState().openConnection("/b.db")
        const descriptorB = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 2")
        await useDbStore.getState().runQuery()
        expect(useDbStore.getState().historyBuckets[descriptorB]).toHaveLength(1)
        expect(useDbStore.getState().historyBuckets[descriptorA]).toHaveLength(1)
    })

    it("never persists new history and reset does not restore raw SQL", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify({
            "/legacy.db": [{ sql: "SELECT raw legacy", ranAt: 1, ok: true, elapsedMs: 1 }]
        }))

        useDbStore.getState().reset()
        expect(useDbStore.getState().historyBuckets).toEqual({})
        expect(JSON.stringify(useDbStore.getState())).not.toContain("SELECT raw legacy")
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).not.toBeNull()
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
        mockRunResultOnce({ kind: "execute", affectedRows: "1", effectOutcome: "unknown" })
        useDbStore.getState().setSql("UPDATE users SET x = 1")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().queries["db-1"].lastSql).toBeNull()
    })

    it("sortResult orders only the loaded page and never dispatches SQL", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockRunResultOnce({
            ...selectResult,
            rows: [
                [{ kind: "integer", value: "2" }],
                [{ kind: "integer", value: "1" }]
            ]
        })
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()
        mockQueryRun.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQueryRun).not.toHaveBeenCalled()
        expect(useDbStore.getState().queries["db-1"].result).toMatchObject({
            rows: [
                [{ kind: "integer", value: "1" }],
                [{ kind: "integer", value: "2" }]
            ]
        })
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 0, dir: "asc" })
    })

    it("sortResult cycles asc → desc → cleared on the same column", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()
        mockQueryRun.mockClear()

        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 0, dir: "asc" })

        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 0, dir: "desc" })

        await useDbStore.getState().sortResult(0)
        expect(useDbStore.getState().queries["db-1"].sortBy).toBeNull()
        expect(mockQueryRun).not.toHaveBeenCalled()
    })

    it("sortResult restarts at asc when a different column is clicked", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users")
        await useDbStore.getState().runQuery()

        await useDbStore.getState().sortResult(0)
        await useDbStore.getState().sortResult(2)

        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({ columnIndex: 2, dir: "asc" })
    })

    it("sortResult never rewrites a trailing semicolon or line comment", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT * FROM users; -- newest\n")
        await useDbStore.getState().runQuery()
        mockQueryRun.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQueryRun).not.toHaveBeenCalled()
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({
            columnIndex: 0,
            dir: "asc"
        })
    })

    it("sortResult is a no-op without a select base", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockRunResultOnce({ kind: "execute", affectedRows: "1", effectOutcome: "unknown" })
        useDbStore.getState().setSql("UPDATE users SET x = 1")
        await useDbStore.getState().runQuery()
        mockQueryRun.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQueryRun).not.toHaveBeenCalled()
    })
})

describe("dbStore dialects", () => {
    it("keeps MSSQL result sorting on the loaded client page", async () => {
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
        mockQueryRun.mockClear()

        await useDbStore.getState().sortResult(0)

        expect(mockQueryRun).not.toHaveBeenCalled()
        expect(useDbStore.getState().queries["db-1"].sortBy).toEqual({
            columnIndex: 0,
            dir: "asc"
        })
    })
})

describe("dbStore saved connections", () => {
    it("openConfig persists a network descriptor without the password and derives key/name", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "db.internal",
            port: 5432,
            database: "app",
            user: "admin",
            password: "s3cr3t",
            ssl: true,
            trustCert: false
        })

        const s = useDbStore.getState()
        expect(s.connections[0]).toEqual({
            connId: "db-1",
            connectionGeneration: expect.any(String),
            kind: "postgres",
            name: "app@db.internal",
            descriptorId: expect.any(String),
            targetKey: "postgres:db.internal:5432:app",
            title: "admin@db.internal:5432/app"
        })
        expect(s.saved).toEqual([
            {
                id: expect.any(String),
                configGeneration: 1,
                targetKey: "postgres:db.internal:5432:app",
                kind: "postgres",
                name: "app@db.internal",
                credentialState: "stored",
                host: "db.internal",
                port: 5432,
                database: "app",
                user: "admin",
                ssl: true,
                trustCert: false
            }
        ])
        // The password must never reach localStorage.
        const raw = localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY) ?? ""
        expect(raw).not.toContain("s3cr3t")
        expect(raw).toBe("")
    })

    it("keys network query history by opaque descriptor", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "h",
            port: 5432,
            database: "d",
            user: "u",
            password: "p",
            ssl: false,
            trustCert: false
        })
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()

        const descriptorId = useDbStore.getState().activeDescriptorId!
        expect(useDbStore.getState().historyBuckets[descriptorId]).toHaveLength(1)
    })

    it("Save and Connect permits a second opaque profile for the same target", async () => {
        await useDbStore.getState().openConnection("/a.db")
        mockProfileCreate.mockClear()

        await useDbStore.getState().openConnection("/a.db")

        expect(mockProfileCreate).toHaveBeenCalledTimes(1)
        expect(useDbStore.getState().connections).toHaveLength(2)
    })

    it("loadSavedConnections keeps postgres trustCert", () => {
        localStorage.setItem(
            DB_CONNECTIONS_STORAGE_KEY,
            JSON.stringify([
                {
                    id: "postgres:h:5432:d",
                    kind: "postgres",
                    name: "d@h",
                    host: "h",
                    port: 5432,
                    database: "d",
                    user: "u",
                    ssl: true,
                    trustCert: true
                }
            ])
        )
        const [saved] = loadSavedConnections()
        expect(saved.kind).toBe("postgres")
        expect(saved.ssl).toBe(true)
        expect(saved.trustCert).toBe(true)
    })

    it("removeSaved forgets the Rust-owned descriptor", async () => {
        await useDbStore.getState().openConnection("/a.db")
        expect(useDbStore.getState().saved).toHaveLength(1)

        const id = useDbStore.getState().saved[0].id
        await useDbStore.getState().removeSaved(id)
        expect(useDbStore.getState().saved).toEqual([])
    })
})

describe("P2 profile repository migration and recovery projection", () => {
    const legacyRecord = {
        id: "legacy-postgres",
        kind: "postgres" as const,
        name: "app@legacy.example",
        host: "legacy.example",
        port: 5432,
        database: "app",
        user: "alice",
        ssl: true,
        trustCert: false
    }

    const storedProfile: DbProfileDescriptor = {
        descriptorId: "profile-ledger" as DbProfileDescriptor["descriptorId"],
        configGeneration: 1,
        name: "app@db.example",
        target: {
            kind: "postgres",
            host: "db.example",
            port: 5432,
            database: "app",
            user: "alice",
            ssl: false,
            trustCert: false
        },
        credentialState: "stored"
    }

    it("imports v1 descriptors before deleting localStorage and never auto-opens a profile", async () => {
        const events: string[] = []
        localStorage.setItem(DB_CONNECTIONS_STORAGE_KEY, JSON.stringify([legacyRecord]))
        mockProfileList.mockImplementationOnce(async () => {
            events.push("list")
            return { profiles: [], recovery: [] }
        })
        mockProfileImportLegacy.mockImplementationOnce(async (request) => {
            events.push("import-confirmed")
            return { profiles: request.profiles, recovery: [] }
        })
        const removeItem = localStorage.removeItem.bind(localStorage)
        vi.spyOn(localStorage, "removeItem").mockImplementation((key) => {
            events.push("legacy-delete")
            expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([
                "legacy-postgres"
            ])
            removeItem(key)
        })

        await useDbStore.getState().initializeProfiles()

        expect(events).toEqual(["list", "import-confirmed", "legacy-delete"])
        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).toBeNull()
        expect(mockProfileImportLegacy).toHaveBeenCalledWith({
            profiles: [expect.objectContaining({
                descriptorId: "legacy-postgres",
                credentialState: "required"
            })]
        })
        expect(mockProfileOpen).not.toHaveBeenCalled()
        expect(mockProfileCreate).not.toHaveBeenCalled()
    })

    it("coalesces StrictMode startup callers and allows a fresh retry after settlement", async () => {
        localStorage.setItem(DB_CONNECTIONS_STORAGE_KEY, JSON.stringify([legacyRecord]))
        let resolveList!: (value: Awaited<ReturnType<typeof dbProfileList>>) => void
        let resolveImport!: (value: Awaited<ReturnType<typeof dbProfileImportLegacy>>) => void
        mockProfileList.mockImplementationOnce(() => new Promise((resolve) => {
            resolveList = resolve
        }))
        mockProfileImportLegacy.mockImplementationOnce(() => new Promise((resolve) => {
            resolveImport = resolve
        }))
        const removeItem = vi.spyOn(localStorage, "removeItem")

        const first = useDbStore.getState().initializeProfiles()
        const strictModeReplay = useDbStore.getState().initializeProfiles()

        expect(strictModeReplay).toBe(first)
        expect(mockProfileList).toHaveBeenCalledTimes(1)
        resolveList({ profiles: [], recovery: [] })
        await vi.waitFor(() => expect(mockProfileImportLegacy).toHaveBeenCalledTimes(1))
        expect(removeItem).not.toHaveBeenCalled()
        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).not.toBeNull()

        const importedProfiles = mockProfileImportLegacy.mock.calls[0][0].profiles
        resolveImport({ profiles: importedProfiles, recovery: [] })
        await Promise.all([first, strictModeReplay])

        expect(mockProfileList).toHaveBeenCalledTimes(1)
        expect(mockProfileImportLegacy).toHaveBeenCalledTimes(1)
        expect(removeItem).toHaveBeenCalledTimes(1)
        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).toBeNull()
        expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([
            "legacy-postgres"
        ])

        // Settlement clears the single-flight cache. A later explicit retry gets
        // a new authoritative snapshot and cannot resurrect the removed v1 key.
        mockProfileList.mockResolvedValueOnce({ profiles: importedProfiles, recovery: [] })
        const retry = useDbStore.getState().initializeProfiles()
        expect(retry).not.toBe(first)
        await retry
        expect(mockProfileList).toHaveBeenCalledTimes(2)
        expect(mockProfileImportLegacy).toHaveBeenCalledTimes(1)
        expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([
            "legacy-postgres"
        ])
    })

    it("keeps the v1 key when Rust import fails and stores only the stable error code", async () => {
        localStorage.setItem(DB_CONNECTIONS_STORAGE_KEY, JSON.stringify([legacyRecord]))
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify({
            "postgres:legacy.example:5432:app": [
                { sql: "SELECT legacy import", ranAt: 1, ok: true, elapsedMs: 1 }
            ]
        }))
        useDbStore.getState().reset()
        mockProfileList.mockResolvedValueOnce({ profiles: [], recovery: [] })
        mockProfileImportLegacy.mockRejectedValueOnce({
            code: "repositoryUnavailable",
            message: "RAW REPOSITORY DETAIL"
        })

        await useDbStore.getState().initializeProfiles()

        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).not.toBeNull()
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).not.toBeNull()
        expect(useDbStore.getState().profileError).toBe("repositoryUnavailable")
        expect(JSON.stringify(useDbStore.getState())).not.toContain("RAW REPOSITORY DETAIL")

        await useDbStore.getState().initializeProfiles()
        expect(mockProfileList).toHaveBeenCalledTimes(2)
        expect(mockProfileImportLegacy).toHaveBeenCalledTimes(2)
        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).toBeNull()
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
        expect(useDbStore.getState().profileError).toBeNull()
        expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([
            "legacy-postgres"
        ])
    })

    it("hydrates the Rust snapshot but retains the v1 key when browser cleanup throws", async () => {
        localStorage.setItem(DB_CONNECTIONS_STORAGE_KEY, JSON.stringify([legacyRecord]))
        vi.spyOn(localStorage, "removeItem").mockImplementation(() => {
            throw new Error("browser storage denied")
        })

        await useDbStore.getState().initializeProfiles()

        expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([
            "legacy-postgres"
        ])
        expect(useDbStore.getState().profileError).toBe("legacyCleanupFailed")
        expect(localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)).not.toBeNull()
    })

    it("startup projects profiles and recovery rows without opening or mutating credentials", async () => {
        mockProfileList.mockResolvedValueOnce({
            profiles: [storedProfile],
            recovery: [{
                operationId: "operation-cleanup",
                descriptorId: storedProfile.descriptorId,
                kind: "cleanupOld",
                allowedActions: ["retryCleanup"]
            }]
        })

        await useDbStore.getState().initializeProfiles()

        expect(useDbStore.getState().recovery).toHaveLength(1)
        expect(mockProfileOpen).not.toHaveBeenCalled()
        expect(mockProfileCreate).not.toHaveBeenCalled()
        expect(mockProfileUpdate).not.toHaveBeenCalled()
        expect(mockProfileRemoveCredential).not.toHaveBeenCalled()
        expect(mockProfileRecover).not.toHaveBeenCalled()
    })

    it("reconciles a lifecycle failure to the backend recovery ledger immediately", async () => {
        useDbStore.setState({
            saved: [{
                id: storedProfile.descriptorId,
                targetKey: "postgres:db.example:5432:app",
                kind: "postgres",
                name: storedProfile.name,
                host: "db.example",
                port: 5432,
                database: "app",
                user: "alice",
                ssl: false,
                trustCert: false,
                credentialState: "stored"
            }],
            connections: [{
                connId: "connection-ledger",
                connectionGeneration: "generation-ledger" as never,
                kind: "postgres",
                name: storedProfile.name,
                descriptorId: storedProfile.descriptorId,
                targetKey: "postgres:db.example:5432:app",
                title: "alice@db.example:5432/app"
            }],
            activeConnId: "connection-ledger",
            tables: { "connection-ledger": usersTable },
            queries: {}
        })
        const rejection = { code: "vaultDeleteFailed", message: "RAW VAULT DETAIL" }
        mockProfileForget.mockRejectedValueOnce(rejection)
        mockProfileList.mockResolvedValueOnce({
            profiles: [storedProfile],
            recovery: [{
                operationId: "operation-forget",
                descriptorId: storedProfile.descriptorId,
                kind: "pendingForget",
                allowedActions: ["retryCleanup"]
            }]
        })

        await expect(useDbStore.getState().removeSaved(storedProfile.descriptorId)).rejects.toBe(rejection)

        const state = useDbStore.getState()
        expect(state.profileError).toBe("vaultDeleteFailed")
        expect(state.recovery).toEqual([expect.objectContaining({ operationId: "operation-forget" })])
        expect(state.connections).toEqual([])
        expect(state.activeConnId).toBeNull()
        expect(state.tables).toEqual({})
        expect(JSON.stringify(state)).not.toContain("RAW VAULT DETAIL")
    })

    it("successful recovery removes stale live/session/table/query ghosts", async () => {
        useDbStore.setState({
            saved: [{
                id: storedProfile.descriptorId,
                targetKey: "postgres:db.example:5432:app",
                kind: "postgres",
                name: storedProfile.name,
                host: "db.example",
                port: 5432,
                database: "app",
                user: "alice",
                credentialState: "stored"
            }],
            recovery: [{
                operationId: "operation-forget",
                descriptorId: storedProfile.descriptorId,
                kind: "pendingForget",
                allowedActions: ["retryCleanup"]
            }],
            connections: [{
                connId: "ghost-connection",
                kind: "postgres",
                name: storedProfile.name,
                descriptorId: storedProfile.descriptorId,
                targetKey: "postgres:db.example:5432:app",
                title: "alice@db.example:5432/app"
            }],
            sessions: {
                [storedProfile.descriptorId]: {
                    descriptorId: storedProfile.descriptorId,
                    connId: "ghost-connection",
                    status: "connected",
                    error: null
                }
            },
            activeConnId: "ghost-connection",
            tables: { "ghost-connection": usersTable },
            queries: {
                "ghost-connection": {
                    sql: "SELECT 1",
                    running: false,
                    result: null,
                    error: null,
                    elapsedMs: null,
                    lastSql: null,
                    sortBy: null,
                    sortBaseRows: null,
                    parseError: null,
                    runGroup: null
                }
            }
        })
        mockProfileRecover.mockResolvedValueOnce({ profiles: [], recovery: [] })

        await useDbStore.getState().recoverProfile({
            operationId: "operation-forget",
            action: "retryCleanup",
            credential: null
        })

        expect(useDbStore.getState()).toMatchObject({
            saved: [],
            recovery: [],
            connections: [],
            sessions: {},
            tables: {},
            queries: {},
            activeConnId: null
        })
    })

    it("never copies a network credential into Zustand or localStorage", async () => {
        const secret = "YUZORA_STORE_SENTINEL"
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "db.example",
            port: 5432,
            database: "app",
            user: "alice",
            password: secret,
            ssl: false,
            trustCert: false
        })

        expect(JSON.stringify(useDbStore.getState())).not.toContain(secret)
        const persisted = Array.from({ length: localStorage.length }, (_, index) => {
            const key = localStorage.key(index)
            return key ? localStorage.getItem(key) : null
        }).join("\n")
        expect(persisted).not.toContain(secret)
        expect(useDbStore.getState().saved[0]).not.toHaveProperty("password")
    })
})

describe("dbStore descriptor identity + sessions", () => {
    it("assigns a stable opaque UUID id (not kind:host:port:db) and derives targetKey", async () => {
        await useDbStore.getState().openConnection("/tmp/app.sqlite")

        const saved = useDbStore.getState().saved[0]
        expect(saved.id).not.toBe("/tmp/app.sqlite")
        expect(saved.id).toMatch(/^dbc-/)
        expect(saved.targetKey).toBe("/tmp/app.sqlite")
    })

    it("creating the same target again keeps separate opaque profile identities", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const id = useDbStore.getState().saved[0].id
        useDbStore.getState().setSql("SELECT 1")
        await useDbStore.getState().runQuery()
        await useDbStore.getState().disconnect(id)

        await useDbStore.getState().openConnection("/a.db")
        expect(useDbStore.getState().saved).toHaveLength(2)
        expect(useDbStore.getState().saved.map((profile) => profile.id)).not.toEqual([id, id])
        expect(useDbStore.getState().historyBuckets[id]).toHaveLength(1)
    })

    it("openConfig records the backend Save and Connect outcome", async () => {
        const descriptorId = "profile-deferred" as DbProfileDescriptor["descriptorId"]
        const outcome = deferred<DbSaveAndConnectOutcome>()
        mockProfileCreate.mockReturnValueOnce(outcome.promise)
        const pending = useDbStore.getState().openConnection("/a.db")
        outcome.resolve({
            outcome: "connected",
            profile: {
                descriptorId,
                configGeneration: 1,
                name: "a.db",
                target: { kind: "sqlite", path: "/a.db" },
                credentialState: "notRequired"
            },
            connection: {
                descriptorId,
                connectionId: "db-9" as never,
                connectionGeneration: "generation-9" as never,
                engine: "sqlite"
            }
        })
        await pending
        const id = useDbStore.getState().saved[0].id
        expect(useDbStore.getState().sessions[id]).toMatchObject({
            status: "connected",
            connId: "db-9",
            error: null
        })
    })

    it("openConfig keeps the profile and records a stable saved-but-connect-failed state", async () => {
        const descriptorId = "profile-failed" as DbProfileDescriptor["descriptorId"]
        mockProfileCreate.mockResolvedValueOnce({
            outcome: "savedButConnectFailed",
            profile: {
                descriptorId,
                configGeneration: 1,
                name: "a.db",
                target: { kind: "sqlite", path: "/a.db" },
                credentialState: "notRequired"
            },
            error: { code: "connectionFailed", message: "database connection failed" }
        })
        const outcome = await useDbStore.getState().openConnection("/a.db")
        expect(outcome.outcome).toBe("savedButConnectFailed")

        const id = useDbStore.getState().saved[0].id
        expect(useDbStore.getState().sessions[id]).toMatchObject({
            status: "error",
            connId: null,
            error: "connectionFailed"
        })
        expect(useDbStore.getState().profileError).toBe("savedButConnectFailed")
        // The failed attempt still leaves no live connection.
        expect(useDbStore.getState().connections).toEqual([])
    })

    it("disconnect keeps the saved descriptor and marks the session disconnected", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const id = useDbStore.getState().saved[0].id

        await useDbStore.getState().disconnect(id)

        const s = useDbStore.getState()
        expect(mockProfileDisconnect).toHaveBeenCalledWith({
            descriptorId: id,
            connectionId: "db-1",
            connectionGeneration: expect.any(String)
        })
        expect(s.connections).toEqual([])
        expect(s.activeConnId).toBeNull()
        // The descriptor survives so the row can reconnect.
        expect(s.saved.map((x) => x.id)).toEqual([id])
        expect(s.sessions[id]).toMatchObject({ status: "disconnected", connId: null })
    })

    it("fails closed when a legacy live projection has no exact generation", async () => {
        const descriptor = {
            id: "legacy-live",
            configGeneration: 1,
            targetKey: "/legacy.db",
            kind: "sqlite" as const,
            name: "legacy.db",
            path: "/legacy.db"
        }
        useDbStore.setState({
            saved: [descriptor],
            connections: [{
                connId: "legacy-connection",
                kind: "sqlite",
                name: "legacy.db",
                descriptorId: descriptor.id,
                targetKey: descriptor.targetKey,
                title: descriptor.path
            }],
            sessions: {
                [descriptor.id]: {
                    descriptorId: descriptor.id,
                    connId: "legacy-connection",
                    status: "connected",
                    error: null
                }
            },
            activeDescriptorId: descriptor.id,
            activeConnId: "legacy-connection"
        })

        expect(await useDbStore.getState().disconnect(descriptor.id)).toBe(false)
        expect(mockProfileDisconnect).not.toHaveBeenCalled()
        expect(useDbStore.getState().connections).toHaveLength(1)
        expect(useDbStore.getState().sessions[descriptor.id]).toMatchObject({
            status: "error",
            error: "staleConnection"
        })
    })

    it("updateSaved rewrites fields in place while keeping the same id", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "old.host",
            port: 5432,
            database: "app",
            user: "admin",
            password: "p",
            ssl: false,
            trustCert: false
        })
        const id = useDbStore.getState().saved[0].id

        await useDbStore.getState().updateSaved(id, {
            kind: "postgres",
            host: "new.host",
            port: 5433,
            database: "app",
            user: "root",
            password: "p",
            ssl: true,
            trustCert: false
        })

        const saved = useDbStore.getState().saved[0]
        expect(saved.id).toBe(id)
        expect(saved.host).toBe("new.host")
        expect(saved.port).toBe(5433)
        expect(saved.user).toBe("root")
        expect(saved.targetKey).toBe("postgres:new.host:5433:app")
        expect(useDbStore.getState().saved).toHaveLength(1)
    })

    it("updateSaved delegates a Keep-Credential target edit to the backend lifecycle (no orphan)", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "old.host",
            port: 5432,
            database: "app",
            user: "admin",
            password: "p",
            ssl: false,
            trustCert: false
        })
        const id = useDbStore.getState().saved[0].id
        expect(useDbStore.getState().sessions[id].status).toBe("connected")

        // Edit the target (host) while connected → the live connection to the old
        // host is now stale and must be closed, not orphaned/leaked.
        await useDbStore.getState().updateSaved(id, {
            kind: "postgres",
            host: "new.host",
            port: 5432,
            database: "app",
            user: "admin",
            password: "",
            ssl: false,
            trustCert: false
        })
        expect(mockProfileUpdate).toHaveBeenCalledWith({
            descriptorId: id,
            name: "app@new.host",
            target: {
                kind: "postgres",
                host: "new.host",
                port: 5432,
                database: "app",
                user: "admin",
                ssl: false,
                trustCert: false
            },
            replacementCredential: null
        })
        // The awaited Rust update owns cancel+close. The frontend must not race
        // it with a second disconnect command; it only projects the returned state.
        expect(mockProfileDisconnect).not.toHaveBeenCalled()

        const s = useDbStore.getState()
        expect(s.connections.find((c) => c.descriptorId === id)).toBeUndefined()
        expect(s.sessions[id]).toMatchObject({ status: "disconnected", connId: null })
        // The descriptor is still updated in place.
        expect(s.saved[0].id).toBe(id)
        expect(s.saved[0].host).toBe("new.host")
    })

    it("pessimistically drops the old live handle when update durability is uncertain", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "old.host",
            port: 5432,
            database: "app",
            user: "admin",
            password: "p",
            ssl: false,
            trustCert: false
        })
        const id = useDbStore.getState().saved[0].id
        useDbStore.getState().setSql("SELECT 1")
        const rejection = {
            code: "repositoryUnavailable",
            message: "RAW PARENT SYNC DETAIL"
        }
        mockProfileUpdate.mockRejectedValueOnce(rejection)
        mockProfileList.mockResolvedValueOnce({
            profiles: [{
                descriptorId: id as DbProfileDescriptor["descriptorId"],
                configGeneration: 2,
                name: "app@new.host",
                target: {
                    kind: "postgres",
                    host: "new.host",
                    port: 5432,
                    database: "app",
                    user: "admin",
                    ssl: false,
                    trustCert: false
                },
                credentialState: "stored"
            }],
            // Simulate ParentSync uncertainty: the durable replacement completed
            // and has no recovery row even though the command returned an error.
            recovery: []
        })

        await expect(useDbStore.getState().updateSaved(id, {
            kind: "postgres",
            host: "new.host",
            port: 5432,
            database: "app",
            user: "admin",
            password: "",
            ssl: false,
            trustCert: false
        })).rejects.toBe(rejection)

        const state = useDbStore.getState()
        expect(state.saved[0].host).toBe("new.host")
        expect(state.connections).toEqual([])
        expect(state.activeConnId).toBeNull()
        expect(state.tables).toEqual({})
        expect(state.queries).toEqual({})
        expect(state.sessions[id]).toEqual({
            descriptorId: id,
            connId: null,
            status: "error",
            error: "repositoryUnavailable"
        })
        expect(state.profileError).toBe("repositoryUnavailable")
        expect(JSON.stringify(state)).not.toContain("RAW PARENT SYNC DETAIL")
    })

    it("updateSaved invalidates a live connection when user or TLS changes", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "h",
            port: 5432,
            database: "app",
            user: "admin",
            password: "p",
            ssl: false,
            trustCert: false
        })
        const id = useDbStore.getState().saved[0].id

        await useDbStore.getState().updateSaved(id, {
            kind: "postgres",
            host: "h",
            port: 5432,
            database: "app",
            user: "root",
            password: "p",
            ssl: true,
            trustCert: false
        })
        await new Promise((r) => setTimeout(r, 0))

        const s = useDbStore.getState()
        expect(s.connections.find((c) => c.descriptorId === id)).toBeUndefined()
        expect(s.sessions[id].status).toBe("disconnected")
        expect(s.saved[0].user).toBe("root")
    })
})

describe("dbStore saved-descriptor open command", () => {
    const sqlite = {
        id: "sqlite-1",
        targetKey: "/a.db",
        kind: "sqlite" as const,
        name: "a.db",
        path: "/a.db"
    }
    const postgres = {
        id: "pg-1",
        targetKey: "postgres:h:5432:d",
        kind: "postgres" as const,
        name: "d@h",
        host: "h",
        port: 5432,
        database: "d",
        user: "u",
        ssl: false
    }

    it("missing descriptor is stale-cancelled and never falls back to the active connection", async () => {
        useDbStore.setState({
            saved: [],
            connections: [{
                connId: "other-live",
                kind: "sqlite",
                name: "other.db",
                descriptorId: "other",
                targetKey: "/other.db",
                title: "/other.db"
            }],
            activeConnId: "other-live",
            reconnectRequest: null
        })

        expect(await useDbStore.getState().openOrReconnectSavedConnection("missing")).toEqual({
            outcome: "cancelled"
        })
        expect(useDbStore.getState().activeConnId).toBe("other-live")
        expect(useDbStore.getState().reconnectRequest).toBeNull()
        expect(mockProfileOpen).not.toHaveBeenCalled()
    })

    it("focuses a live non-active descriptor, then stale-cancels when it is already active", async () => {
        useDbStore.setState({
            saved: [sqlite],
            connections: [
                {
                    connId: "sqlite-live",
                    kind: "sqlite",
                    name: "a.db",
                    descriptorId: sqlite.id,
                    targetKey: sqlite.targetKey,
                    title: sqlite.path
                },
                {
                    connId: "other-live",
                    kind: "sqlite",
                    name: "other.db",
                    descriptorId: "other",
                    targetKey: "/other.db",
                    title: "/other.db"
                }
            ],
            activeConnId: "other-live",
            tables: { "sqlite-live": [], "other-live": [] }
        })

        expect(await useDbStore.getState().openOrReconnectSavedConnection(sqlite.id)).toEqual({
            outcome: "completed"
        })
        expect(useDbStore.getState().activeConnId).toBe("sqlite-live")
        expect(await useDbStore.getState().openOrReconnectSavedConnection(sqlite.id)).toEqual({
            outcome: "cancelled"
        })
        expect(mockProfileOpen).not.toHaveBeenCalled()
    })

    it("cancels a connecting descriptor without opening or requesting another flow", async () => {
        useDbStore.setState({
            saved: [postgres],
            connections: [],
            sessions: {
                [postgres.id]: {
                    descriptorId: postgres.id,
                    connId: null,
                    status: "connecting",
                    error: null
                }
            },
            reconnectRequest: null
        })

        expect(await useDbStore.getState().openOrReconnectSavedConnection(postgres.id)).toEqual({
            outcome: "cancelled"
        })
        expect(useDbStore.getState().reconnectRequest).toBeNull()
        expect(mockProfileOpen).not.toHaveBeenCalled()
    })

    it("opens an offline SQLite descriptor directly and preserves its descriptor identity", async () => {
        useDbStore.setState({ saved: [sqlite], connections: [], activeConnId: null })

        expect(await useDbStore.getState().openOrReconnectSavedConnection(sqlite.id)).toEqual({
            outcome: "completed"
        })
        expect(mockProfileOpen).toHaveBeenCalledWith(sqlite.id)
        expect(useDbStore.getState().connections[0]).toMatchObject({
            connId: "db-1",
            descriptorId: sqlite.id
        })
        expect(useDbStore.getState().sessions[sqlite.id]).toMatchObject({
            status: "connected",
            error: null
        })
    })

    it("returns a SQLite error as data while keeping the row session in error state", async () => {
        useDbStore.setState({ saved: [sqlite], connections: [], activeConnId: null })
        mockProfileOpen.mockRejectedValueOnce(new Error("file is locked"))

        const result = await useDbStore.getState().openOrReconnectSavedConnection(sqlite.id)

        expect(result).toMatchObject({ outcome: "error", error: expect.any(Error) })
        expect(useDbStore.getState().sessions[sqlite.id]).toEqual({
            descriptorId: sqlite.id,
            connId: null,
            status: "error",
            error: "unknown"
        })
        expect(useDbStore.getState().connections).toEqual([])
    })

    it("requests a prefilled network reconnect with a new token on every trigger", async () => {
        useDbStore.setState({
            saved: [postgres],
            connections: [],
            activeConnId: null,
            sessions: {},
            reconnectRequest: null
        })
        mockProfileOpen.mockRejectedValue({
            code: "credentialRequired",
            message: "credential required"
        })

        expect(await useDbStore.getState().openOrReconnectSavedConnection(postgres.id)).toEqual({
            outcome: "completed"
        })
        expect(useDbStore.getState().reconnectRequest).toEqual({
            descriptorId: postgres.id,
            token: 1
        })

        expect(await useDbStore.getState().openOrReconnectSavedConnection(postgres.id)).toEqual({
            outcome: "completed"
        })
        expect(useDbStore.getState().reconnectRequest).toEqual({
            descriptorId: postgres.id,
            token: 2
        })
        expect(mockProfileOpen).toHaveBeenCalledTimes(2)
    })
})

describe("P3 descriptor-owned connection orchestration", () => {
    const savedSqlite = (id: string, path: string) => ({
        id,
        configGeneration: 1,
        targetKey: path,
        kind: "sqlite" as const,
        name: path.split("/").pop() ?? path,
        path,
        credentialState: "notRequired" as const
    })

    it("keeps same-target profiles with different users and TLS in independent descriptor buckets", async () => {
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "db.example",
            port: 5432,
            database: "app",
            user: "alice",
            password: "alice-secret",
            ssl: false,
            trustCert: false
        })
        await useDbStore.getState().openConfig({
            kind: "postgres",
            host: "db.example",
            port: 5432,
            database: "app",
            user: "bob",
            password: "bob-secret",
            ssl: true,
            trustCert: true
        })

        const [alice, bob] = useDbStore.getState().saved
        expect(alice.targetKey).toBe(bob.targetKey)
        expect(alice.id).not.toBe(bob.id)
        expect(useDbStore.getState().connections).toHaveLength(2)

        useDbStore.getState().setActiveDescriptor(alice.id)
        useDbStore.getState().setSql("SELECT 'alice'")
        await useDbStore.getState().runQuery()
        expect(useDbStore.getState().historyBuckets[bob.id] ?? []).toEqual([])
        useDbStore.getState().setActiveDescriptor(bob.id)
        useDbStore.getState().setSql("SELECT 'bob'")
        await useDbStore.getState().runQuery()

        expect(useDbStore.getState().queryBuckets[alice.id].sql).toBe("SELECT 'alice'")
        expect(useDbStore.getState().queryBuckets[bob.id].sql).toBe("SELECT 'bob'")
        expect(useDbStore.getState().historyBuckets[alice.id][0].sql).toBe("SELECT 'alice'")
        expect(useDbStore.getState().historyBuckets[bob.id][0].sql).toBe("SELECT 'bob'")
        expect(Object.keys(useDbStore.getState().tableBuckets).sort()).toEqual(
            [alice.id, bob.id].sort()
        )
    })

    it("discards legacy target history after hydrate without copying raw SQL into session buckets", async () => {
        const targetKey = "postgres:db.example:5432:app"
        const legacy = [{ sql: "SELECT legacy", ranAt: 1, ok: true, elapsedMs: 2 }]
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify({ [targetKey]: legacy }))
        const getItem = vi.spyOn(localStorage, "getItem")
        useDbStore.getState().reset()
        const profile = (descriptorId: string, user: string): DbProfileDescriptor => ({
            descriptorId: descriptorId as DbProfileDescriptor["descriptorId"],
            configGeneration: 1,
            name: `app@db.example (${user})`,
            target: {
                kind: "postgres",
                host: "db.example",
                port: 5432,
                database: "app",
                user,
                ssl: user === "bob",
                trustCert: false
            },
            credentialState: "stored"
        })
        const listing = deferred<Awaited<ReturnType<typeof dbProfileList>>>()
        mockProfileList.mockReturnValueOnce(listing.promise)

        const initializing = useDbStore.getState().initializeProfiles()
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).not.toBeNull()
        getItem.mockClear()
        expect(useDbStore.getState().historyBuckets).toEqual({})
        listing.resolve({
            profiles: [profile("profile-alice", "alice"), profile("profile-bob", "bob")],
            recovery: []
        })
        await initializing

        const hydrated = useDbStore.getState().historyBuckets
        expect(hydrated["profile-alice"]).toEqual([])
        expect(hydrated["profile-bob"]).toEqual([])
        expect(JSON.stringify(useDbStore.getState())).not.toContain("SELECT legacy")
        expect(getItem).not.toHaveBeenCalledWith(DB_HISTORY_STORAGE_KEY)
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()

        useDbStore.getState().recordHistory("profile-alice", {
            sql: "SELECT alice only",
            ranAt: 2,
            ok: true,
            elapsedMs: 1
        })
        expect(useDbStore.getState().historyBuckets["profile-alice"].map((entry) => entry.sql)).toEqual([
            "SELECT alice only"
        ])
        expect(useDbStore.getState().historyBuckets["profile-bob"]).toEqual([])
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
    })

    it("retains legacy history after profile-list failure and clears it on retry", async () => {
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify({
            "/a.db": [{ sql: "SELECT retry", ranAt: 1, ok: true, elapsedMs: 1 }]
        }))
        useDbStore.getState().reset()
        mockProfileList.mockRejectedValueOnce({
            code: "repositoryUnavailable",
            message: "raw repository detail"
        })

        await useDbStore.getState().initializeProfiles()
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).not.toBeNull()
        expect(useDbStore.getState().historyBuckets).toEqual({})

        mockProfileList.mockResolvedValueOnce({ profiles: [], recovery: [] })
        await useDbStore.getState().initializeProfiles()
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).toBeNull()
    })

    it("surfaces only a stable error when legacy history cleanup fails", async () => {
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify({
            "/a.db": [{ sql: "SELECT cleanup", ranAt: 1, ok: true, elapsedMs: 1 }]
        }))
        useDbStore.getState().reset()
        const removeItem = localStorage.removeItem.bind(localStorage)
        vi.spyOn(localStorage, "removeItem").mockImplementation((key) => {
            if (key === DB_HISTORY_STORAGE_KEY) throw new Error("RAW STORAGE DETAIL")
            removeItem(key)
        })
        mockProfileList.mockResolvedValueOnce({ profiles: [], recovery: [] })

        await useDbStore.getState().initializeProfiles()

        expect(useDbStore.getState().profileError).toBe("legacyHistoryCleanupFailed")
        expect(JSON.stringify(useDbStore.getState())).not.toContain("RAW STORAGE DETAIL")
        expect(localStorage.getItem(DB_HISTORY_STORAGE_KEY)).not.toBeNull()
    })

    it("does not let A completion activate after the later B intent", async () => {
        const a = savedSqlite("profile-a", "/a.db")
        const b = savedSqlite("profile-b", "/b.db")
        useDbStore.setState({ saved: [a, b], connections: [], activeDescriptorId: null, activeConnId: null })
        const openA = deferred<DbLiveConnection>()
        const openB = deferred<DbLiveConnection>()
        mockProfileOpen.mockImplementation((descriptorId) =>
            descriptorId === a.id ? openA.promise : openB.promise
        )

        const pendingA = useDbStore.getState().openOrReconnectSavedConnection(a.id)
        const pendingB = useDbStore.getState().openOrReconnectSavedConnection(b.id)
        openA.resolve(live(a.id, "connection-a", "generation-a"))
        await pendingA

        expect(useDbStore.getState().connections.map((connection) => connection.descriptorId)).toContain(a.id)
        expect(useDbStore.getState().activeDescriptorId).toBeNull()

        openB.resolve(live(b.id, "connection-b", "generation-b"))
        await pendingB
        expect(useDbStore.getState().activeDescriptorId).toBe(b.id)
        expect(useDbStore.getState().activeConnId).toBe("connection-b")
    })

    it("keeps the later B intent active when two pending opens complete in reverse order", async () => {
        const a = savedSqlite("profile-a", "/a.db")
        const b = savedSqlite("profile-b", "/b.db")
        useDbStore.setState({ saved: [a, b], connections: [], activeDescriptorId: null, activeConnId: null })
        const openA = deferred<DbLiveConnection>()
        const openB = deferred<DbLiveConnection>()
        mockProfileOpen.mockImplementation((descriptorId) =>
            descriptorId === a.id ? openA.promise : openB.promise
        )

        const pendingA = useDbStore.getState().openOrReconnectSavedConnection(a.id)
        const pendingB = useDbStore.getState().openOrReconnectSavedConnection(b.id)
        openB.resolve(live(b.id, "connection-b", "generation-b"))
        await pendingB
        openA.resolve(live(a.id, "connection-a", "generation-a"))
        await pendingA

        expect(useDbStore.getState().activeDescriptorId).toBe(b.id)
        expect(useDbStore.getState().activeConnId).toBe("connection-b")
        expect(useDbStore.getState().liveMru).toEqual([b.id, a.id])
    })

    it("uses live-only MRU fallback C to B to A", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        await useDbStore.getState().openConnection("/c.db")
        const [a, b, c] = useDbStore.getState().saved.map((profile) => profile.id)

        await useDbStore.getState().disconnect(c)
        expect(useDbStore.getState().activeDescriptorId).toBe(b)
        await useDbStore.getState().disconnect(b)
        expect(useDbStore.getState().activeDescriptorId).toBe(a)
        expect(useDbStore.getState().liveMru).toEqual([a])
    })

    it("does not move active selection when an inactive descriptor closes", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        const [a, b] = useDbStore.getState().saved.map((profile) => profile.id)

        await useDbStore.getState().disconnect(a)

        expect(useDbStore.getState().activeDescriptorId).toBe(b)
        expect(useDbStore.getState().activeConnId).toBe("db-2")
    })

    it("exact-disconnects a removed descriptor's late open without creating a ghost", async () => {
        const descriptor = savedSqlite("profile-remove", "/remove.db")
        useDbStore.setState({ saved: [descriptor], connections: [] })
        const pendingLive = deferred<DbLiveConnection>()
        mockProfileOpen.mockReturnValueOnce(pendingLive.promise)

        const opening = useDbStore.getState().openOrReconnectSavedConnection(descriptor.id)
        await useDbStore.getState().removeSaved(descriptor.id)
        const late = live(descriptor.id, "late-connection", "late-generation")
        pendingLive.resolve(late)
        await opening

        expect(mockProfileDisconnect).toHaveBeenCalledWith(late)
        expect(useDbStore.getState().saved).toEqual([])
        expect(useDbStore.getState().connections).toEqual([])
        expect(useDbStore.getState().sessions[descriptor.id]).toBeUndefined()
        expect(useDbStore.getState().tableBuckets[descriptor.id]).toBeUndefined()
    })

    it("exact-disconnects a pre-edit late open and keeps the new config generation", async () => {
        const descriptor = savedSqlite("profile-edit", "/before.db")
        useDbStore.setState({ saved: [descriptor], connections: [] })
        const pendingLive = deferred<DbLiveConnection>()
        mockProfileOpen.mockReturnValueOnce(pendingLive.promise)

        const opening = useDbStore.getState().openOrReconnectSavedConnection(descriptor.id)
        await useDbStore.getState().updateSaved(descriptor.id, { kind: "sqlite", path: "/after.db" })
        const late = live(descriptor.id, "old-connection", "old-generation")
        pendingLive.resolve(late)
        await opening

        expect(mockProfileDisconnect).toHaveBeenCalledWith(late)
        expect(useDbStore.getState().saved[0]).toMatchObject({
            id: descriptor.id,
            configGeneration: 2,
            path: "/after.db"
        })
        expect(useDbStore.getState().connections).toEqual([])
    })

    it("ignores a generation-1 disconnect completion after generation 2 is live", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const pendingDisconnect = deferred<void>()
        mockProfileDisconnect.mockReturnValueOnce(pendingDisconnect.promise)
        const disconnecting = useDbStore.getState().disconnect(descriptorId)

        const generation2 = {
            ...useDbStore.getState().connections[0],
            connId: "db-generation-2",
            connectionGeneration: "generation-2" as never
        }
        useDbStore.setState({
            connections: [generation2],
            activeDescriptorId: descriptorId,
            activeConnId: generation2.connId,
            sessions: {
                [descriptorId]: {
                    descriptorId,
                    connId: generation2.connId,
                    status: "connected",
                    error: null
                }
            }
        })
        pendingDisconnect.resolve()
        await disconnecting

        expect(useDbStore.getState().connections).toEqual([generation2])
        expect(useDbStore.getState().activeConnId).toBe(generation2.connId)
    })

    it("ignores generation-1 metadata after generation 2 refreshes", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const generation1Tables = deferred<DbTable[]>()
        mockList.mockReturnValueOnce(generation1Tables.promise)
        const staleRefresh = useDbStore.getState().loadTables(descriptorId)

        const generation2 = {
            ...useDbStore.getState().connections[0],
            connId: "db-generation-2",
            connectionGeneration: "generation-2" as never
        }
        useDbStore.setState({
            connections: [generation2],
            activeConnId: generation2.connId,
            sessions: {
                [descriptorId]: {
                    descriptorId,
                    connId: generation2.connId,
                    status: "connected",
                    error: null
                }
            }
        })
        const generation2Tables: DbTable[] = [
            { catalog: "main", schema: "main", name: "generation_2", kind: "table" }
        ]
        mockList.mockResolvedValueOnce(generation2Tables)
        await useDbStore.getState().loadTables(descriptorId)
        generation1Tables.resolve([
            { catalog: "main", schema: "main", name: "generation_1", kind: "table" }
        ])
        await staleRefresh

        expect(useDbStore.getState().tableBuckets[descriptorId]).toEqual(generation2Tables)
    })

    it("ignores generation-1 query completion after a generation-2 run", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        useDbStore.getState().setSql("SELECT 'generation 1'")
        const generation1Result = deferred<DbQueryRun>()
        let generation1Request!: DbQueryRunRequest
        mockQueryRun.mockImplementationOnce((request) => {
            generation1Request = request
            return generation1Result.promise
        })
        const staleRun = useDbStore.getState().runQuery()

        const generation2 = {
            ...useDbStore.getState().connections[0],
            connId: "db-generation-2",
            connectionGeneration: "generation-2" as never
        }
        useDbStore.setState((state) => ({
            connections: [generation2],
            activeConnId: generation2.connId,
            queryBuckets: {
                ...state.queryBuckets,
                [descriptorId]: {
                    ...state.queryBuckets[descriptorId],
                    sql: "SELECT 'generation 2'",
                    running: false
                }
            }
        }))
        const generation2Result: DbQueryResult = {
            ...selectResult,
            rows: [[{ kind: "integer", value: "2" }]]
        }
        mockRunResultOnce(generation2Result)
        await useDbStore.getState().runQuery()
        generation1Result.resolve(queryRunFromResult(generation1Request, {
            ...selectResult,
            rows: [[{ kind: "integer", value: "1" }]]
        }))
        await staleRun

        expect(useDbStore.getState().queryBuckets[descriptorId].result).toEqual(generation2Result)
    })

    it("publishes only the newest same-generation table refresh", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const first = deferred<DbTable[]>()
        const second = deferred<DbTable[]>()
        mockList.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)

        const firstRefresh = useDbStore.getState().loadTables(descriptorId)
        const secondRefresh = useDbStore.getState().loadTables(descriptorId)
        const newest: DbTable[] = [
            { catalog: "main", schema: "main", name: "newest", kind: "table" }
        ]
        second.resolve(newest)
        await secondRefresh
        first.resolve([{ catalog: "main", schema: "main", name: "older", kind: "table" }])
        await firstRefresh

        expect(useDbStore.getState().tableBuckets[descriptorId]).toEqual(newest)
    })

    it("publishes only the newest exact-owner columns completion", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const table = usersTable[0]
        const first = deferred<DbColumn[]>()
        const second = deferred<DbColumn[]>()
        mockColumns.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)

        const firstLoad = useDbStore.getState().loadColumns(descriptorId, table)
        const secondLoad = useDbStore.getState().loadColumns(descriptorId, table)
        const newest: DbColumn[] = [
            { name: "newest", type: "TEXT", notnull: false, pk: false }
        ]
        second.resolve(newest)
        await secondLoad
        first.resolve([{ name: "older", type: "TEXT", notnull: false, pk: false }])
        await firstLoad

        expect(useDbStore.getState().columnBuckets[descriptorId][dbObjectRefKey(table)]).toEqual(newest)
        expect(mockColumns).toHaveBeenCalledWith(expect.objectContaining({
            descriptorId,
            connectionId: "db-1",
            connectionGeneration: expect.any(String)
        }), table)
    })

    it("publishes concurrent column loads for distinct full object references independently", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const publicUsers = usersTable[0]
        const auditUsers: DbTable = {
            catalog: "main",
            schema: "audit",
            name: "users",
            kind: "table"
        }
        const publicLoad = deferred<DbColumn[]>()
        const auditLoad = deferred<DbColumn[]>()
        mockColumns.mockReturnValueOnce(publicLoad.promise).mockReturnValueOnce(auditLoad.promise)

        const first = useDbStore.getState().loadColumns(descriptorId, publicUsers)
        const second = useDbStore.getState().loadColumns(descriptorId, auditUsers)
        const auditColumns: DbColumn[] = [
            { name: "audit_id", type: "UUID", notnull: true, pk: true }
        ]
        const publicColumns: DbColumn[] = [
            { name: "id", type: "INTEGER", notnull: true, pk: true }
        ]
        auditLoad.resolve(auditColumns)
        await second
        publicLoad.resolve(publicColumns)
        await first

        const buckets = useDbStore.getState().columnBuckets[descriptorId]
        expect(buckets[dbObjectRefKey(publicUsers)]).toEqual(publicColumns)
        expect(buckets[dbObjectRefKey(auditUsers)]).toEqual(auditColumns)
    })

    it("preserves columns and structured error state on metadata failure or connectionBusy", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const table = usersTable[0]
        const existing: DbColumn[] = [
            { name: "id", type: "INTEGER", notnull: true, pk: true }
        ]
        mockColumns.mockResolvedValueOnce(existing)
        await useDbStore.getState().loadColumns(descriptorId, table)

        mockColumns.mockRejectedValueOnce({ code: "metadataFailed", message: "raw detail" })
        await useDbStore.getState().loadColumns(descriptorId, table)
        expect(useDbStore.getState().columnBuckets[descriptorId][dbObjectRefKey(table)]).toEqual(existing)
        expect(useDbStore.getState().columnErrors[descriptorId][dbObjectRefKey(table)]).toEqual({
            code: "metadataFailed",
            databaseError: null
        })

        mockColumns.mockRejectedValueOnce({ code: "connectionBusy", message: "lease held" })
        await useDbStore.getState().loadColumns(descriptorId, table)
        expect(useDbStore.getState().columnBuckets[descriptorId][dbObjectRefKey(table)]).toEqual(existing)
        expect(useDbStore.getState().columnErrors[descriptorId][dbObjectRefKey(table)]).toEqual({
            code: "connectionBusy",
            databaseError: null
        })
        expect(JSON.stringify(useDbStore.getState())).not.toContain("raw detail")
    })

    it("turns only the exact columns owner offline on serverDisconnected", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        const [a, b] = useDbStore.getState().saved.map((saved) => saved.id)
        mockColumns.mockRejectedValueOnce({ code: "serverDisconnected", message: "socket closed" })

        await useDbStore.getState().loadColumns(b, usersTable[0])

        expect(useDbStore.getState().sessions[b]).toMatchObject({
            status: "disconnected",
            connId: null,
            error: "serverDisconnected"
        })
        expect(useDbStore.getState().activeDescriptorId).toBe(a)
        expect(useDbStore.getState().connections.map((connection) => connection.descriptorId)).toEqual([a])
    })

    it("preserves metadata while exposing a structured retryable failure", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        mockList.mockRejectedValueOnce({ code: "metadataFailed", message: "raw driver detail" })

        await useDbStore.getState().loadTables(descriptorId)

        expect(useDbStore.getState().tableBuckets[descriptorId]).toEqual(usersTable)
        expect(useDbStore.getState().tableErrors[descriptorId]).toEqual({
            code: "metadataFailed",
            databaseError: null
        })
        expect(JSON.stringify(useDbStore.getState())).not.toContain("raw driver detail")
    })

    it("keeps the object tree when metadata reports connectionBusy", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        mockList.mockRejectedValueOnce({ code: "connectionBusy", message: "lease held" })

        await useDbStore.getState().loadTables(descriptorId)

        expect(useDbStore.getState().tableBuckets[descriptorId]).toEqual(usersTable)
        expect(useDbStore.getState().tableErrors[descriptorId]).toEqual({
            code: "connectionBusy",
            databaseError: null
        })
    })

    it("turns an exact server disconnect offline, applies MRU fallback, and reconnects", async () => {
        await useDbStore.getState().openConnection("/a.db")
        await useDbStore.getState().openConnection("/b.db")
        const [a, b] = useDbStore.getState().saved.map((profile) => profile.id)
        mockList.mockRejectedValueOnce({ code: "serverDisconnected", message: "socket closed" })

        await useDbStore.getState().loadTables(b)

        expect(useDbStore.getState().sessions[b]).toMatchObject({
            status: "disconnected",
            connId: null,
            error: "serverDisconnected"
        })
        expect(useDbStore.getState().activeDescriptorId).toBe(a)
        expect(useDbStore.getState().connections.map((connection) => connection.descriptorId)).toEqual([a])

        await useDbStore.getState().openOrReconnectSavedConnection(b)
        expect(useDbStore.getState().sessions[b]).toMatchObject({ status: "connected" })
        expect(useDbStore.getState().activeDescriptorId).toBe(b)
    })

    it("keeps a typed SQLite missing-path failure offline without a synthetic handle", async () => {
        const descriptorId = "sqlite-missing" as DbProfileDescriptor["descriptorId"]
        mockProfileCreate.mockResolvedValueOnce({
            outcome: "savedButConnectFailed",
            profile: {
                descriptorId,
                configGeneration: 1,
                name: "missing.db",
                target: { kind: "sqlite", path: "/missing/missing.db" },
                credentialState: "notRequired"
            },
            error: { code: "sqlitePathMissing", message: "path does not exist" }
        })

        const outcome = await useDbStore.getState().openConnection("/missing/missing.db")

        expect(outcome.outcome).toBe("savedButConnectFailed")
        expect(mockProfileOpen).not.toHaveBeenCalled()
        expect(useDbStore.getState().connections).toEqual([])
        expect(useDbStore.getState().sessions[descriptorId]).toMatchObject({
            status: "error",
            error: "sqlitePathMissing"
        })
    })
})

/** P1 records deterministic ownership for regressions whose behavior belongs to
 * a later approved phase. These are intentionally non-red seams: removing the
 * owner prefix or enabling one before its phase supplies the implementation is
 * a review error, not a reason to weaken the expected behavior. */
describe("approved owner-tagged database regression seams", () => {
    it("[P4 owner] same object name in two schemas keeps qualified identity and columns", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const publicUsers: DbTable = {
            catalog: "app",
            schema: "public",
            name: "users",
            kind: "table"
        }
        const auditUsers: DbTable = { ...publicUsers, schema: "audit" }
        const publicColumns: DbColumn[] = [
            { name: "public_id", type: "bigint", notnull: true, pk: true }
        ]
        const auditColumns: DbColumn[] = [
            { name: "audit_id", type: "uuid", notnull: true, pk: true }
        ]
        mockColumns
            .mockResolvedValueOnce(publicColumns)
            .mockResolvedValueOnce(auditColumns)

        await useDbStore.getState().loadColumns(descriptorId, publicUsers)
        await useDbStore.getState().loadColumns(descriptorId, auditUsers)

        const buckets = useDbStore.getState().columnBuckets[descriptorId]
        expect(dbObjectRefKey(publicUsers)).not.toBe(dbObjectRefKey(auditUsers))
        expect(buckets[dbObjectRefKey(publicUsers)]).toEqual(publicColumns)
        expect(buckets[dbObjectRefKey(auditUsers)]).toEqual(auditColumns)
        expect(mockColumns).toHaveBeenNthCalledWith(
            1,
            expect.any(Object),
            publicUsers
        )
        expect(mockColumns).toHaveBeenNthCalledWith(
            2,
            expect.any(Object),
            auditUsers
        )
    })

    it("[P4 owner] PostgreSQL and MSSQL BIGINT/decimal values stay lossless end to end", async () => {
        const lossless: DbQueryResult = {
            kind: "select",
            columns: ["big", "decimal"],
            rows: [[
                { kind: "integer", value: "9223372036854775807" },
                { kind: "decimal", value: "-0.1200" }
            ]],
            truncated: false,
            affectedRows: null,
            effectOutcome: "unknown"
        }
        for (const kind of ["postgres", "mssql"] as const) {
            const target: DbOpenConfig = kind === "postgres"
                ? {
                      kind,
                      host: "db.example",
                      port: 5432,
                      database: "app",
                      user: "alice",
                      password: "secret",
                      ssl: false,
                      trustCert: false
                  }
                : {
                      kind,
                      host: "db.example",
                      port: 1433,
                      database: "app",
                      user: "alice",
                      password: "secret",
                      trustCert: false
                  }
            await useDbStore.getState().openConfig(target)
            mockRunResultOnce(lossless)
            useDbStore.getState().setSql("SELECT big, decimal_value")

            await useDbStore.getState().runQuery()

            const descriptorId = useDbStore.getState().activeDescriptorId!
            expect(useDbStore.getState().queryBuckets[descriptorId].result).toEqual(lossless)
        }
    })
    it("[P6 owner] quoted -- SQL executes the exact unit without sorting rewrite", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const sql = "SELECT '-- literal; still SQL' AS value;\nSELECT 2;"
        useDbStore.getState().setSql(sql)
        let request!: DbQueryRunRequest
        mockQueryRun.mockImplementationOnce(async (nextRequest) => {
            request = nextRequest
            return queryRunFromResult(nextRequest, selectResult)
        })
        const cursor = sql.indexOf("literal")

        await useDbStore.getState().runQuery({
            kind: "primary",
            selection: { from: cursor, to: cursor },
            cursor
        })

        expect(request.mode).toBe("primary")
        expect(request.statements).toEqual([{
            sql: "SELECT '-- literal; still SQL' AS value;",
            transactionBoundary: "none"
        }])
        expect(request.statements[0].sql).not.toContain("ORDER BY")
    })
    it("[P6 owner] multi-statement run sends parser units in order with transaction metadata", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("BEGIN;\nSELECT 1;\nCOMMIT;")

        await useDbStore.getState().runQuery({ kind: "all" })

        expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
            mode: "script",
            statements: [
                { sql: "BEGIN;", transactionBoundary: "begin" },
                { sql: "SELECT 1;", transactionBoundary: "none" },
                { sql: "COMMIT;", transactionBoundary: "commit" }
            ]
        }))
    })

    it("cancels only the exact active run owner and coalesces a second click", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;")
        const runDeferred = deferred<DbQueryRun>()
        let request!: DbQueryRunRequest
        mockQueryRun.mockImplementationOnce((nextRequest) => {
            request = nextRequest
            return runDeferred.promise
        })
        const cancelDeferred = deferred<DbQueryCancelResult>()
        mockQueryCancel.mockReturnValueOnce(cancelDeferred.promise)

        const pendingRun = useDbStore.getState().runQuery()
        await vi.waitFor(() => expect(useDbStore.getState().queryBuckets[request.descriptorId].running).toBe(true))
        const firstCancel = useDbStore.getState().cancelQuery()
        const secondCancel = useDbStore.getState().cancelQuery()

        expect(mockQueryCancel).toHaveBeenCalledTimes(1)
        expect(mockQueryCancel).toHaveBeenCalledWith({
            descriptorId: request.descriptorId,
            connectionId: request.connectionId,
            connectionGeneration: request.connectionGeneration,
            queryRunId: request.queryRunId
        })
        expect(useDbStore.getState().queryBuckets[request.descriptorId].runGroup?.status)
            .toBe("cancelling")

        cancelDeferred.resolve({ outcome: "cancelled" })
        await Promise.all([firstCancel, secondCancel])
        runDeferred.resolve(queryRunFromResult(request, selectResult))
        await pendingRun
        expect(useDbStore.getState().queryBuckets[request.descriptorId].runGroup?.status)
            .toBe("settled")
    })

    it("keeps cancelled and skipped statement state with the cancelled tab active", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("UPDATE a SET n = 1;\nSELECT slow();\nUPDATE b SET n = 2;")
        const cancelledError = {
            engine: "sqlite" as const,
            message: "interrupted",
            code: "SQLITE_INTERRUPT",
            position: null,
            detail: null,
            hint: null,
            retryability: "retryable" as const
        }
        mockQueryRun.mockImplementationOnce(async (request) => {
            const cancelledId = `${request.queryRunId}:statement:1` as never
            return queryRunWithStatements(request, [
                executeExecution(request, 0, "committed"),
                {
                    statementExecutionId: cancelledId,
                    statementIndex: 1,
                    sql: request.statements[1].sql,
                    effectOutcome: "unknown",
                    result: { kind: "cancelled", error: cancelledError }
                },
                {
                    statementExecutionId: `${request.queryRunId}:statement:2` as never,
                    statementIndex: 2,
                    sql: request.statements[2].sql,
                    effectOutcome: "none",
                    result: { kind: "skipped" }
                }
            ])
        })

        await useDbStore.getState().runQuery({ kind: "all" })

        const descriptorId = useDbStore.getState().activeDescriptorId!
        const group = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        expect(group.run?.statements.map((statement) => statement.result.kind)).toEqual([
            "execute",
            "cancelled",
            "skipped"
        ])
        expect(group.activeStatementExecutionId).toBe(group.run?.statements[1].statementExecutionId)
    })

    it("keeps the settled run while a terminated cancellation moves only its profile offline", async () => {
        await useDbStore.getState().openConnection("/a.db")
        const descriptorId = useDbStore.getState().activeDescriptorId!
        mockList.mockClear()
        useDbStore.getState().setSql("SELECT slow();")
        const runDeferred = deferred<DbQueryRun>()
        let request!: DbQueryRunRequest
        mockQueryRun.mockImplementationOnce((nextRequest) => {
            request = nextRequest
            return runDeferred.promise
        })
        mockQueryCancel.mockResolvedValueOnce({ outcome: "cancelledConnectionTerminated" })

        const pendingRun = useDbStore.getState().runQuery()
        await vi.waitFor(() => expect(useDbStore.getState().queryBuckets[descriptorId].running).toBe(true))
        await useDbStore.getState().cancelQuery()
        expect(useDbStore.getState().connections).toHaveLength(1)
        runDeferred.resolve(queryRunWithStatements(
            request,
            [executeExecution(request, 0, "committed")]
        ))
        await pendingRun

        expect(mockList).not.toHaveBeenCalled()
        expect(useDbStore.getState().connections).toHaveLength(0)
        expect(useDbStore.getState().sessions[descriptorId]).toMatchObject({
            status: "disconnected",
            error: null
        })
        expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.run?.queryRunId)
            .toBe(request.queryRunId)
    })

    it("rejects an unsafe script before IPC and preserves the previous run group", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;")
        await useDbStore.getState().runQuery({ kind: "all" })
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const previousRunId = useDbStore.getState().queryBuckets[descriptorId].runGroup?.owner.queryRunId
        mockQueryRun.mockClear()
        useDbStore.getState().setSql("SELECT 1; SELECT 'unterminated")

        await useDbStore.getState().runQuery({ kind: "all" })

        const query = useDbStore.getState().queryBuckets[descriptorId]
        expect(mockQueryRun).not.toHaveBeenCalled()
        expect(query.parseError?.code).toBe("unterminated-string")
        expect(query.runGroup?.owner.queryRunId).toBe(previousRunId)
    })

    it("releases the prior result session before replacing the descriptor run group", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;")
        await useDbStore.getState().runQuery({ kind: "all" })
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const firstGroup = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        const firstOwner = firstGroup.run!.statements[0].result.kind === "rows"
            ? firstGroup.run!.statements[0].result.resultSession!.owner
            : null
        mockResultSessionRelease.mockClear()
        useDbStore.getState().setSql("SELECT 2;")

        await useDbStore.getState().runQuery({ kind: "all" })

        expect(mockResultSessionRelease).toHaveBeenCalledWith(firstOwner)
        expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.owner.queryRunId)
            .not.toBe(firstGroup.owner.queryRunId)
    })

    it("keeps old tabs during stale-generation cleanup and still starts the new run", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;")
        await useDbStore.getState().runQuery({ kind: "all" })
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const firstGroup = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        const release = deferred<DbResultPage>()
        mockResultSessionRelease.mockReturnValueOnce(release.promise)
        mockQueryRun.mockClear()
        useDbStore.setState((state) => ({
            connections: state.connections.map((connection) => connection.descriptorId === descriptorId
                ? {
                      ...connection,
                      connId: "db-reconnected",
                      connectionGeneration: "generation-2" as never
                  }
                : connection),
            activeConnId: "db-reconnected"
        }))
        useDbStore.getState().setSql("SELECT 2;")

        const pending = useDbStore.getState().runQuery({ kind: "all" })
        await vi.waitFor(() => expect(mockResultSessionRelease).toHaveBeenCalled())
        expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.owner.queryRunId)
            .toBe(firstGroup.owner.queryRunId)
        expect(mockQueryRun).not.toHaveBeenCalled()
        release.reject({ code: "staleConnection", message: "old generation" })
        await pending

        expect(mockQueryRun).toHaveBeenCalledTimes(1)
        expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.owner.queryRunId)
            .not.toBe(firstGroup.owner.queryRunId)
    })

    it("keeps the old run and exposes a non-stale release failure", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;")
        await useDbStore.getState().runQuery({ kind: "all" })
        const descriptorId = useDbStore.getState().activeDescriptorId!
        const firstGroup = useDbStore.getState().queryBuckets[descriptorId].runGroup!
        mockResultSessionRelease.mockRejectedValueOnce({
            code: "queryFailed",
            message: "release failed"
        })
        mockQueryRun.mockClear()
        useDbStore.getState().setSql("SELECT 2;")

        await useDbStore.getState().runQuery({ kind: "all" })

        const query = useDbStore.getState().queryBuckets[descriptorId]
        expect(mockQueryRun).not.toHaveBeenCalled()
        expect(query.runGroup?.owner.queryRunId).toBe(firstGroup.owner.queryRunId)
        expect(query.error?.code).toBe("queryFailed")
        expect(query.running).toBe(false)
    })
    it("[P6 owner] row-producing and following DML project as ordered statement tabs", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("SELECT 1;\nUPDATE counters SET value = value + 1;")
        mockQueryRun.mockImplementationOnce(async (request) => queryRunWithStatements(
            request,
            [rowExecution(request, 0), executeExecution(request, 1)]
        ))

        await useDbStore.getState().runQuery({ kind: "all" })

        const descriptorId = useDbStore.getState().activeDescriptorId!
        const group = useDbStore.getState().queryBuckets[descriptorId].runGroup
        expect(group?.status).toBe("settled")
        expect(group?.run?.statements.map((statement) => statement.result.kind)).toEqual([
            "rows",
            "execute"
        ])
        expect(group?.run?.statements.map((statement) => statement.sql)).toEqual([
            "SELECT 1;",
            "UPDATE counters SET value = value + 1;"
        ])
    })

    it("[P6 owner] BEGIN SELECT COMMIT preserves explicit transaction boundaries", async () => {
        await useDbStore.getState().openConnection("/a.db")
        useDbStore.getState().setSql("BEGIN;\nSELECT 1;\nCOMMIT;")
        let request!: DbQueryRunRequest
        mockQueryRun.mockImplementationOnce(async (nextRequest) => {
            request = nextRequest
            return queryRunWithStatements(nextRequest, [
                executeExecution(nextRequest, 0, "transactionPending"),
                rowExecution(nextRequest, 1),
                executeExecution(nextRequest, 2, "committed")
            ])
        })

        await useDbStore.getState().runQuery({ kind: "all" })

        expect(request.statements.map((statement) => statement.transactionBoundary)).toEqual([
            "begin",
            "none",
            "commit"
        ])
        const descriptorId = useDbStore.getState().activeDescriptorId!
        expect(useDbStore.getState().queryBuckets[descriptorId].runGroup?.run)
            .toMatchObject({ transactionMayBeOpen: false })
    })
})
