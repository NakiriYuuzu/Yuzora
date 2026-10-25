import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { DatabaseSync } from "node:sqlite"
import type { DbLiveConnection, DbQueryRun, DbStatementExecutionId } from "./src/lib/types"
import { dbObjectRefKey } from "./src/lib/databaseSql"

vi.mock("@/lib/ipc", () => ({
    dbListTables: vi.fn(), dbTableColumns: vi.fn(), dbQueryRun: vi.fn(),
    dbQueryCancel: vi.fn(), dbResultPagePrevious: vi.fn(), dbResultPageNext: vi.fn(),
    dbResultSessionRelease: vi.fn(), dbProfileList: vi.fn(), dbProfileImportLegacy: vi.fn(),
    dbProfileCreate: vi.fn(), dbProfileUpdate: vi.fn(), dbProfileRemoveCredential: vi.fn(),
    dbProfileForget: vi.fn(), dbProfileRecover: vi.fn(), dbProfileOpen: vi.fn(), dbProfileDisconnect: vi.fn()
}))

import { dbListTables, dbTableColumns, dbQueryRun } from "./src/lib/ipc"
import { useDbStore } from "./src/state/dbStore"

// Node's SQLite fixture stays outside the browser TypeScript project. Only the
// IPC transport is replaced; catalog reads and SQL execute against this database.
let fixture: DatabaseSync
const descriptorId = "fixture081"

beforeEach(() => {
    fixture = new DatabaseSync(":memory:")
    vi.clearAllMocks()
    localStorage.clear()
    useDbStore.getState().reset()
    useDbStore.setState({
        saved: [{ id: descriptorId, configGeneration: 1, targetKey: "fixture081", kind: "sqlite", name: "fixture", path: "/fixture081.db" }],
        connections: [{ connId: "db-fixture081", connectionGeneration: "generation081" as DbLiveConnection["connectionGeneration"], kind: "sqlite", name: "fixture", descriptorId, targetKey: "fixture081", title: "fixture" }],
        activeDescriptorId: descriptorId,
        activeConnId: "db-fixture081"
    })
    vi.mocked(dbListTables).mockImplementation(async () => fixture.prepare("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view')").all().map(row => ({
        catalog: "main", schema: "main", name: String(row.name), kind: row.type === "view" ? "view" : "table"
    })))
    vi.mocked(dbTableColumns).mockImplementation(async (_, table) => fixture.prepare(`PRAGMA table_xinfo("${table.name.replaceAll('"', '""')}")`).all().map(column => ({
        name: String(column.name), type: String(column.type), notnull: Boolean(column.notnull), pk: Boolean(column.pk)
    })))
    vi.mocked(dbQueryRun).mockImplementation(async request => {
        expect(request.statements).toHaveLength(1)
        const statement = request.statements[0]
        fixture.exec(statement.sql)
        const statements: DbQueryRun["statements"] = [{
            statementExecutionId: `${request.queryRunId}:0` as DbStatementExecutionId,
            statementIndex: 0, sql: statement.sql, effectOutcome: "committed",
            result: { kind: "execute", affectedRows: "0" }
        }]
        return { ...request, statements, transactionMayBeOpen: false, connectionTerminated: false }
    })
})

afterEach(() => fixture.close())

describe("database catalog real SQLite changes", () => {
    it.each([
        ["ALTER TABLE users ADD COLUMN label TEXT", ["id", "label"]],
        ["ALTER TABLE users RENAME COLUMN id TO renamed", ["renamed"]]
    ] as const)("refreshes cached metadata after %s", async (sql, names) => {
        fixture.exec("CREATE TABLE users (id INTEGER PRIMARY KEY)")
        await useDbStore.getState().loadTables(descriptorId)
        const table = useDbStore.getState().tableBuckets[descriptorId][0]
        await useDbStore.getState().loadColumns(descriptorId, table)
        expect(dbTableColumns).toHaveBeenCalledTimes(1)
        useDbStore.getState().setSql(sql)
        await useDbStore.getState().runQuery()
        await vi.waitFor(() => expect(useDbStore.getState().columnBuckets[descriptorId][dbObjectRefKey(table)].map(column => column.name)).toEqual(names))
        expect(dbTableColumns).toHaveBeenCalledTimes(2)
    })

    it("refreshes a recreated same-name view", async () => {
        fixture.exec("CREATE VIEW report AS SELECT 1 AS original")
        await useDbStore.getState().loadTables(descriptorId)
        const view = useDbStore.getState().tableBuckets[descriptorId][0]
        await useDbStore.getState().loadColumns(descriptorId, view)
        fixture.exec("DROP VIEW report; CREATE VIEW report AS SELECT 2 AS replacement")
        await useDbStore.getState().loadTables(descriptorId)
        expect(useDbStore.getState().columnBuckets[descriptorId][dbObjectRefKey(view)].map(column => column.name)).toEqual(["replacement"])
    })
})
