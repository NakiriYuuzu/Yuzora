import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { open as openFileDialog } from "@tauri-apps/plugin-dialog"

import type {
  DbLiveConnection,
  DbProfileDescriptor,
  DbProfileTarget,
  DbQueryRun,
  DbQueryRunRequest,
} from "@/lib/types"
import { dbObjectRefKey } from "@/lib/databaseSql"
import i18n from "@/lib/i18n"
import type { DbConnection, DbHistoryEntry, SavedDbConnection } from "@/state/dbStore"

vi.mock("@/lib/ipc", () => ({
  dbListTables: vi.fn(),
  dbTableColumns: vi.fn(),
  dbQueryRun: vi.fn(),
  dbQueryCancel: vi.fn(),
  dbResultSessionRelease: vi.fn(),
  dbProfileList: vi.fn(),
  dbProfileImportLegacy: vi.fn(),
  dbProfileCreate: vi.fn(),
  dbProfileUpdate: vi.fn(),
  dbProfileRemoveCredential: vi.fn(),
  dbProfileForget: vi.fn(),
  dbProfileRecover: vi.fn(),
  dbProfileOpen: vi.fn(),
  dbProfileDisconnect: vi.fn(),
  dbTestConnection: vi.fn()
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))

import {
  dbListTables,
  dbTableColumns,
  dbProfileCreate,
  dbProfileForget,
  dbProfileImportLegacy,
  dbProfileList,
  dbProfileOpen,
  dbProfileRecover,
  dbProfileRemoveCredential,
  dbProfileUpdate,
  dbQueryRun,
  dbTestConnection
} from "@/lib/ipc"
import { useDbStore } from "@/state/dbStore"
import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"

const mockList = vi.mocked(dbListTables)
const mockColumns = vi.mocked(dbTableColumns)
const mockOpenFileDialog = vi.mocked(openFileDialog)
const mockQueryRun = vi.mocked(dbQueryRun)
const mockProfileList = vi.mocked(dbProfileList)
const mockProfileImportLegacy = vi.mocked(dbProfileImportLegacy)
const mockProfileCreate = vi.mocked(dbProfileCreate)
const mockProfileUpdate = vi.mocked(dbProfileUpdate)
const mockProfileRemoveCredential = vi.mocked(dbProfileRemoveCredential)
const mockProfileForget = vi.mocked(dbProfileForget)
const mockProfileRecover = vi.mocked(dbProfileRecover)
const mockProfileOpen = vi.mocked(dbProfileOpen)
const mockTestConnection = vi.mocked(dbTestConnection)

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage (mirrors sshStore.test.ts).
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

/** A SQLite live connection whose descriptor id doubles as the path for test
 *  convenience (the real id is an opaque UUID; identity is opaque either way). */
function sqliteConn(connId: string, path: string): DbConnection {
  const name = path.split("/").pop() ?? path
  return {
    connId,
    connectionGeneration: `generation-${connId}` as never,
    kind: "sqlite",
    name,
    descriptorId: path,
    targetKey: path,
    title: path
  }
}
function savedFor(conn: DbConnection): SavedDbConnection {
  return {
    id: conn.descriptorId,
    configGeneration: 1,
    targetKey: conn.targetKey,
    kind: "sqlite",
    name: conn.name,
    path: conn.targetKey
  }
}

function emptyRowsRun(request: DbQueryRunRequest): DbQueryRun {
  return {
    descriptorId: request.descriptorId,
    connectionId: request.connectionId,
    connectionGeneration: request.connectionGeneration,
    queryRunId: request.queryRunId,
    statements: request.statements.map((statement, statementIndex) => ({
      statementExecutionId: `${request.queryRunId}:statement:${statementIndex}` as never,
      statementIndex,
      sql: statement.sql,
      effectOutcome: "none" as const,
      result: statementIndex === 0
        ? { kind: "rows" as const, resultSession: null, affectedRows: null }
        : { kind: "skipped" as const },
    })) as unknown as DbQueryRun["statements"],
    transactionMayBeOpen: false,
    connectionTerminated: false,
  }
}

function storedPostgres(id: string): SavedDbConnection {
  return {
    id,
    configGeneration: 1,
    targetKey: "postgres:h:5432:d",
    kind: "postgres",
    name: "d@h",
    host: "h",
    port: 5432,
    database: "d",
    user: "u",
    ssl: false,
    trustCert: false,
    credentialState: "stored"
  }
}

function profilesFromStore(): DbProfileDescriptor[] {
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

function seed(opts: {
  connections?: DbConnection[]
  saved?: SavedDbConnection[]
  activeConnId?: string | null
  historyByTarget?: Record<string, DbHistoryEntry[]>
}) {
  const connections = opts.connections ?? [sqliteConn("db-1", "/tmp/app.sqlite")]
  const saved = opts.saved ?? connections.map(savedFor)
  const activeConnId = opts.activeConnId ?? connections[0]?.connId ?? null
  const activeDescriptorId = connections.find((connection) => connection.connId === activeConnId)
    ?.descriptorId ?? null
  const historyByTarget = opts.historyByTarget ?? {}
  useDbStore.setState({
    connections,
    saved,
    activeDescriptorId,
    activeConnId,
    liveMru: activeDescriptorId
      ? [activeDescriptorId, ...connections.map((connection) => connection.descriptorId)
        .filter((descriptorId) => descriptorId !== activeDescriptorId)]
      : [],
    tableBuckets: Object.fromEntries(connections.map((connection) => [connection.descriptorId, []])),
    tables: Object.fromEntries(connections.map((c) => [c.connId, []])),
    historyBuckets: Object.fromEntries(connections.flatMap((connection) => {
      const entries = historyByTarget[connection.targetKey]
      return entries ? [[connection.descriptorId, entries]] : []
    }))
  })
}

async function fillNewPostgresForm(password: string): Promise<void> {
  fireEvent.click(screen.getByText("New connection…"))
  fireEvent.click(screen.getByText("PostgreSQL"))
  fireEvent.change(screen.getByLabelText("Host"), { target: { value: "db.example" } })
  fireEvent.change(screen.getByLabelText("Database"), { target: { value: "app" } })
  fireEvent.change(screen.getByLabelText("User"), { target: { value: "alice" } })
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: password } })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  useDbStore.getState().reset()
  vi.clearAllMocks()
  mockList.mockResolvedValue([])
  mockColumns.mockResolvedValue([])
  mockQueryRun.mockImplementation(async (request) => emptyRowsRun(request))
  mockProfileList.mockImplementation(async () => ({ profiles: profilesFromStore(), recovery: [] }))
  mockProfileImportLegacy.mockImplementation(async (request) => ({ profiles: request.profiles, recovery: [] }))
  mockProfileCreate.mockImplementation(async (request) => {
    const descriptorId = "profile-new" as DbProfileDescriptor["descriptorId"]
    const profile: DbProfileDescriptor = {
      descriptorId,
      configGeneration: 1,
      name: request.name,
      target: request.target,
      credentialState: request.target.kind === "sqlite" ? "notRequired" : "stored"
    }
    return {
      outcome: "connected",
      profile,
      connection: {
        descriptorId,
        connectionId: "db-new" as never,
        connectionGeneration: "generation-new" as never,
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
    profiles: profilesFromStore().filter((profile) => profile.descriptorId !== descriptorId),
    recovery: []
  }))
  mockProfileRemoveCredential.mockImplementation(async (descriptorId) => ({
    profiles: profilesFromStore().map((profile) => profile.descriptorId === descriptorId
      ? { ...profile, credentialState: "required" as const }
      : profile),
    recovery: []
  }))
  mockProfileRecover.mockImplementation(async () => ({ profiles: profilesFromStore(), recovery: [] }))
  let openSequence = 0
  mockProfileOpen.mockImplementation(async (descriptorId) => {
    const profile = profilesFromStore().find((item) => item.descriptorId === descriptorId)
    if (!profile) throw { code: "profileNotFound", message: "missing" }
    return {
      descriptorId,
      connectionId: `db-open-${++openSequence}` as never,
      connectionGeneration: `generation-open-${openSequence}` as never,
      engine: profile.target.kind
    }
  })
  mockTestConnection.mockResolvedValue({ elapsedMs: 3, serverVersion: "test-db" })
})

afterEach(() => {
  cleanup()
})

describe("DatabaseNavContent recent queries", () => {
  it("renders the recent-queries list with sql first line, relative time and a failure dot", () => {
    const now = Date.now()
    seed({
      historyByTarget: {
        "/tmp/app.sqlite": [
          { sql: "SELECT 1\nFROM users", ranAt: now, ok: true, elapsedMs: 3 },
          { sql: "DROP TABLE gone", ranAt: now, ok: false, error: "no such table: gone", elapsedMs: 1 }
        ]
      }
    })
    render(<DatabaseNavContent />)

    expect(screen.getByText("Recent queries")).toBeInTheDocument()
    // Multi-line sql collapses to its first line in the row.
    expect(screen.getByText("SELECT 1")).toBeInTheDocument()
    expect(screen.getByText("DROP TABLE gone")).toBeInTheDocument()
    // The failed run surfaces a labelled red dot.
    expect(screen.getByLabelText("Query failed")).toBeInTheDocument()
    // A just-now run reads as "now".
    expect(screen.getAllByText("now").length).toBeGreaterThan(0)
  })

  it("clicking a history entry fills the editor sql but does not run it", () => {
    seed({
      historyByTarget: {
        "/tmp/app.sqlite": [{ sql: "DELETE FROM users", ranAt: Date.now(), ok: true, elapsedMs: 2 }]
      }
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("DELETE FROM users"))

    expect(useDbStore.getState().queries["db-1"].sql).toBe("DELETE FROM users")
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("keeps the bounded recent-queries region empty when the active connection has no history", () => {
    seed({ historyByTarget: {} })
    render(<DatabaseNavContent />)
    expect(screen.getByTestId("db-history-region")).toBeInTheDocument()
    expect(screen.queryByTestId("db-history-row")).not.toBeInTheDocument()
  })

  it("only shows history for the active connection's key", () => {
    seed({
      connections: [sqliteConn("db-1", "/a.db"), sqliteConn("db-2", "/b.db")],
      activeConnId: "db-1",
      historyByTarget: {
        "/a.db": [{ sql: "SELECT 'a'", ranAt: Date.now(), ok: true, elapsedMs: 1 }],
        "/b.db": [{ sql: "SELECT 'b'", ranAt: Date.now(), ok: true, elapsedMs: 1 }]
      }
    })
    render(<DatabaseNavContent />)

    expect(screen.getByText("SELECT 'a'")).toBeInTheDocument()
    expect(screen.queryByText("SELECT 'b'")).not.toBeInTheDocument()
  })

  it("never falls back to shared target history when the active descriptor has no runtime bucket", () => {
    const targetKey = "postgres:db.example:5432:app"
    const aliceConnection: DbConnection = {
      connId: "connection-alice",
      connectionGeneration: "generation-alice" as never,
      descriptorId: "profile-alice",
      targetKey,
      kind: "postgres",
      name: "app@db.example",
      title: "alice@db.example:5432/app"
    }
    const bobConnection: DbConnection = {
      ...aliceConnection,
      connId: "connection-bob",
      connectionGeneration: "generation-bob" as never,
      descriptorId: "profile-bob",
      title: "bob@db.example:5432/app"
    }
    const profile = (id: string, user: string, ssl: boolean): SavedDbConnection => ({
      id,
      configGeneration: 1,
      targetKey,
      kind: "postgres",
      name: "app@db.example",
      host: "db.example",
      port: 5432,
      database: "app",
      user,
      ssl,
      trustCert: false,
      credentialState: "stored"
    })
    const aliceHistory: DbHistoryEntry[] = [{
      sql: "SELECT alice_private",
      ranAt: Date.now(),
      ok: true,
      elapsedMs: 1
    }]
    useDbStore.setState({
      connections: [aliceConnection, bobConnection],
      saved: [profile("profile-alice", "alice", false), profile("profile-bob", "bob", true)],
      activeDescriptorId: "profile-bob",
      activeConnId: "connection-bob",
      liveMru: ["profile-bob", "profile-alice"],
      historyBuckets: { "profile-alice": aliceHistory }
    })

    render(<DatabaseNavContent />)

    expect(screen.queryByText("SELECT alice_private")).not.toBeInTheDocument()
    expect(screen.queryByTestId("db-history-row")).not.toBeInTheDocument()
  })
})

describe("DatabaseNavContent P5 bounded regions", () => {
  it("gives each region its own overflow anchor and collapses recent queries independently", () => {
    seed({
      historyByTarget: {
        "/tmp/app.sqlite": [{ sql: "SELECT 1", ranAt: Date.now(), ok: true, elapsedMs: 1 }]
      }
    })
    render(<DatabaseNavContent />)

    expect(screen.getByTestId("db-nav-root")).toHaveClass("overflow-hidden")
    expect(screen.getByTestId("db-region-grid")).toHaveClass(
      "grid-rows-[minmax(40px,0.8fr)_minmax(40px,1.4fr)_minmax(24px,0.7fr)]"
    )
    expect(screen.getByTestId("db-saved-region")).toHaveClass("min-h-0", "overflow-hidden")
    expect(screen.getByTestId("db-saved-scroll")).toHaveClass("min-h-0", "overflow-y-auto")
    expect(screen.getByTestId("db-object-region")).toHaveClass("min-h-0", "overflow-hidden")
    expect(screen.getByTestId("db-object-scroll")).toHaveClass("min-h-0", "overflow-y-auto")
    expect(screen.getByTestId("db-history-region")).toHaveClass("max-h-[168px]", "overflow-hidden")
    expect(screen.getByTestId("db-history-scroll")).toHaveClass("overflow-y-auto")
    expect(screen.getByTestId("db-new-connection")).toHaveClass("shrink-0")

    const toggle = screen.getByTestId("db-history-toggle")
    expect(toggle).toHaveAttribute("aria-expanded", "true")
    fireEvent.click(toggle)
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(screen.queryByTestId("db-history-scroll")).not.toBeInTheDocument()
    expect(screen.getByTestId("db-saved-scroll")).toBeInTheDocument()
    expect(screen.getByTestId("db-object-scroll")).toBeInTheDocument()
  })

  it("projects an explicit semantic state on every row and exposes the active row accessibly", () => {
    const makeSqlite = (id: string, name: string): SavedDbConnection => ({
      id,
      configGeneration: 1,
      targetKey: `/tmp/${name}`,
      kind: "sqlite",
      name,
      path: `/tmp/${name}`,
      credentialState: "notRequired"
    })
    const connected = sqliteConn("connection-connected", "/tmp/connected.db")
    connected.descriptorId = "connected"
    useDbStore.setState({
      connections: [connected],
      saved: [
        makeSqlite("offline", "offline.db"),
        makeSqlite("connecting", "connecting.db"),
        { ...makeSqlite("connected", "connected.db"), targetKey: "/tmp/connected.db" },
        makeSqlite("error", "error.db"),
        { ...storedPostgres("credential"), name: "credential.db", credentialState: "required" },
        { ...storedPostgres("vault"), name: "vault.db", credentialState: "unavailable" }
      ],
      activeDescriptorId: "connected",
      activeConnId: "connection-connected",
      liveMru: ["connected"],
      sessions: {
        offline: { descriptorId: "offline", connId: null, status: "disconnected", error: null },
        connecting: { descriptorId: "connecting", connId: null, status: "connecting", error: null },
        connected: { descriptorId: "connected", connId: "connection-connected", status: "connected", error: null },
        error: { descriptorId: "error", connId: null, status: "error", error: "connectionFailed" },
        credential: { descriptorId: "credential", connId: null, status: "disconnected", error: null },
        vault: { descriptorId: "vault", connId: null, status: "disconnected", error: null }
      }
    })
    render(<DatabaseNavContent />)

    const rows = screen.getAllByTestId("db-saved-row")
    expect(rows.map((row) => row.getAttribute("data-status"))).toEqual([
      "offline",
      "connecting",
      "connected",
      "error",
      "offline",
      "offline"
    ])
    expect(rows[4]).toHaveAttribute("data-credential-state", "credentialRequired")
    expect(rows[5]).toHaveAttribute("data-credential-state", "vaultUnavailable")
    expect(rows[2].querySelector('button[aria-current="true"]')).toHaveTextContent("connected.db")
    expect(within(rows[4]).getByText(/credential required/)).toBeInTheDocument()
    expect(within(rows[5]).getByText(/vault unavailable/)).toBeInTheDocument()
    for (const row of rows) expect(within(row).getByText(/Offline|Connecting|Connected|Error/)).toBeInTheDocument()
  })
})

describe("DatabaseNavContent P3 metadata recovery", () => {
  const users = [{ catalog: "main", schema: "main", name: "users", kind: "table" as const }]

  it("keeps existing tables visible and retries a structured metadata failure", async () => {
    mockList.mockResolvedValueOnce(users)
    await useDbStore.getState().openConnection("/a.db")
    const descriptorId = useDbStore.getState().activeDescriptorId!
    mockList.mockRejectedValueOnce({ code: "metadataFailed", message: "raw detail" })
    await useDbStore.getState().loadTables(descriptorId)
    render(<DatabaseNavContent />)

    expect(screen.getByText("users")).toBeInTheDocument()
    expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh database objects.")
    expect(screen.queryByText("raw detail")).not.toBeInTheDocument()

    const refreshed = [{ catalog: "main", schema: "main", name: "projects", kind: "table" as const }]
    mockList.mockResolvedValueOnce(refreshed)
    fireEvent.click(screen.getByRole("button", { name: "Retry object refresh" }))

    expect(await screen.findByText("projects")).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByText("Could not refresh database objects.")).not.toBeInTheDocument())
  })

  it("keeps the tree and shows Release or Cancel guidance for connectionBusy", async () => {
    mockList.mockResolvedValueOnce(users)
    await useDbStore.getState().openConnection("/a.db")
    const descriptorId = useDbStore.getState().activeDescriptorId!
    mockList.mockRejectedValueOnce({ code: "connectionBusy", message: "lease held" })
    await useDbStore.getState().loadTables(descriptorId)
    render(<DatabaseNavContent />)

    expect(screen.getByText("users")).toBeInTheDocument()
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Read the result to the end, Release Result, or Cancel before retrying."
    )
    expect(screen.getByRole("button", { name: "Retry object refresh" })).toBeInTheDocument()
  })

  it("keeps duplicate object names keyed and queried by their full reference", async () => {
    const duplicateUsers = [
      { catalog: "main", schema: "main", name: "users", kind: "table" as const },
      { catalog: "audit", schema: "audit", name: "users", kind: "table" as const }
    ]
    mockList.mockResolvedValueOnce(duplicateUsers)
    mockColumns.mockResolvedValueOnce([
      { name: "id", type: "BIGINT", notnull: true, pk: true }
    ])
    await useDbStore.getState().openConnection("/a.db")
    render(<DatabaseNavContent />)

    expect(screen.getByRole("button", { name: "Catalog: main" })).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByRole("button", { name: "Schema: audit" })).toBeInTheDocument()
    expect(screen.getAllByRole("button", { name: "Tables" })).toHaveLength(2)
    expect(screen.getAllByText("users")).toHaveLength(2)
    fireEvent.click(screen.getByTitle("audit.audit.users"))

    await waitFor(() => expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
      statements: [{
        sql: 'SELECT * FROM "audit"."users" LIMIT 100',
        transactionBoundary: "none",
      }]
    })))

    const auditRef = dbObjectRefKey(duplicateUsers[1])
    const auditRow = screen.getAllByTestId("db-object-row").find(
      (row) => row.getAttribute("data-object-ref") === auditRef
    )!
    fireEvent.click(within(auditRow).getByRole("button", { name: "Expand columns for users" }))
    await waitFor(() => expect(mockColumns).toHaveBeenCalledWith(
      expect.objectContaining({ descriptorId: expect.any(String) }),
      duplicateUsers[1]
    ))
    expect(await within(auditRow).findByText("id")).toBeInTheDocument()
    expect(within(auditRow).getByText("BIGINT")).toBeInTheDocument()
    expect(within(auditRow).getByText("NOT NULL")).toBeInTheDocument()
    expect(within(auditRow).getByText("PK")).toBeInTheDocument()
  })

  it("shows an inline retry when one object's column load fails", async () => {
    mockList.mockResolvedValueOnce(users)
    mockColumns.mockRejectedValueOnce({ code: "metadataFailed", message: "RAW COLUMN DETAIL" })
    await useDbStore.getState().openConnection("/a.db")
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByRole("button", { name: "Expand columns for users" }))
    expect(await screen.findByText("Could not load columns.")).toBeInTheDocument()
    expect(screen.queryByText("RAW COLUMN DETAIL")).not.toBeInTheDocument()

    mockColumns.mockResolvedValueOnce([
      { name: "email", type: "TEXT", notnull: false, pk: false }
    ])
    fireEvent.click(screen.getByRole("button", { name: "Retry columns for users" }))
    expect(await screen.findByText("email")).toBeInTheDocument()
    expect(screen.getByText("NULL")).toBeInTheDocument()
  })
})

describe("DatabaseNavContent saved connections", () => {
  it("shows a saved-but-offline descriptor and clicking it reconnects a SQLite file directly", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{ id: "/x.db", targetKey: "/x.db", kind: "sqlite", name: "x.db", path: "/x.db" }],
      activeConnId: null
    })
    render(<DatabaseNavContent />)

    expect(screen.getByText("x.db")).toBeInTheDocument()
    expect(screen.getByText(/not connected/)).toBeInTheDocument()

    fireEvent.click(screen.getByText("x.db"))
    // SQLite reconnect opens directly (no dialog / password prompt).
    await waitFor(() => expect(mockProfileOpen).toHaveBeenCalledWith("/x.db"))
    expect(useDbStore.getState().sessions["/x.db"]).toMatchObject({
      status: "connected",
      error: null
    })
  })

  it("reconnects a stored network credential without opening a dialog", async () => {
    const profile = storedPostgres("pg-zero-dialog")
    useDbStore.setState({
      connections: [],
      saved: [profile],
      activeDescriptorId: null,
      activeConnId: null,
      sessions: {}
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))

    await waitFor(() => expect(mockProfileOpen).toHaveBeenCalledWith("pg-zero-dialog"))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(useDbStore.getState().sessions["pg-zero-dialog"]).toMatchObject({
      status: "connected",
      error: null
    })
  })

  it("keeps a failed direct SQLite open visible on its saved row", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{ id: "/locked.db", targetKey: "/locked.db", kind: "sqlite", name: "locked.db", path: "/locked.db" }],
      activeConnId: null
    })
    mockProfileOpen.mockRejectedValueOnce(new Error("file is locked"))
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("locked.db"))

    const badge = await screen.findByText("Error")
    expect(badge).toHaveAttribute("title", "The database operation could not be completed.")
    expect(useDbStore.getState().sessions["/locked.db"]).toEqual({
      descriptorId: "/locked.db",
      connId: null,
      status: "error",
      error: "unknown"
    })
  })

  it("opens credential recovery only after a saved profile reports credentialRequired", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "postgres:h:5432:d",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null
    })
    mockProfileOpen.mockRejectedValueOnce({
      code: "credentialRequired",
      message: "credential required"
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))

    expect(await screen.findByText("Reconnect")).toBeInTheDocument()
    expect(useDbStore.getState().reconnectRequestToken).toBe(1)
    expect(useDbStore.getState().reconnectRequest).toBeNull()
    // Only the password is asked for — the connect button drives openConfig.
    expect(screen.getByText("Connect")).toBeInTheDocument()
    expect(mockProfileCreate).not.toHaveBeenCalled()
  })

  it("consumes repeated reconnect requests for the same descriptor as fresh dialog instances", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "pg-repeat",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null
    })
    mockProfileOpen.mockRejectedValue({
      code: "credentialRequired",
      message: "credential required"
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))
    const firstPassword = await screen.findByLabelText("Password")
    fireEvent.change(firstPassword, { target: { value: "first-attempt" } })
    expect(firstPassword).toHaveValue("first-attempt")

    await act(async () => {
      expect(await useDbStore.getState().openOrReconnectSavedConnection("pg-repeat")).toEqual({
        outcome: "completed"
      })
    })

    await waitFor(() => expect(screen.getByLabelText("Password")).toHaveValue(""))
    expect(useDbStore.getState().reconnectRequestToken).toBe(2)
    expect(useDbStore.getState().reconnectRequest).toBeNull()
    expect(screen.getByText("Reconnect")).toBeInTheDocument()
  })

  it("does not consume a reconnect request when the nav unmounts before its effect settles", async () => {
    const saved: SavedDbConnection = {
      id: "pg-remount",
      targetKey: "postgres:h:5432:d",
      kind: "postgres",
      name: "d@h",
      host: "h",
      port: 5432,
      database: "d",
      user: "u",
      ssl: false
    }
    useDbStore.setState({
      connections: [],
      saved: [saved],
      activeConnId: null,
      reconnectRequest: { descriptorId: saved.id, token: 1 },
      reconnectRequestToken: 1
    })

    const first = render(<DatabaseNavContent />)
    first.unmount()
    expect(useDbStore.getState().reconnectRequest).toEqual({
      descriptorId: saved.id,
      token: 1
    })

    render(<DatabaseNavContent />)
    expect(await screen.findByText("Reconnect")).toBeInTheDocument()
    expect(useDbStore.getState().reconnectRequest).toBeNull()
  })

  it("row Open uses the shared command to focus a live non-active descriptor", async () => {
    const a = sqliteConn("db-a", "/a.db")
    const b = sqliteConn("db-b", "/b.db")
    seed({ connections: [a, b], activeConnId: "db-b" })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("a.db"))

    await waitFor(() => expect(useDbStore.getState().activeConnId).toBe("db-a"))
    expect(mockProfileOpen).not.toHaveBeenCalled()
  })

  it("keeps the database navigation usable after dismissing a failed reconnect", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "pg-uuid",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null
    })
    mockProfileOpen
      .mockRejectedValueOnce({ code: "credentialRequired", message: "credential required" })
      .mockRejectedValueOnce(new Error("connection refused"))
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))
    fireEvent.change(await screen.findByLabelText("Password"), { target: { value: "bad-password" } })
    fireEvent.click(screen.getByText("Connect"))

    expect(await screen.findByText("The database operation could not be completed.")).toBeInTheDocument()
    fireEvent.click(screen.getByText("Cancel"))

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(screen.getByText("d@h")).toBeInTheDocument()
    expect(screen.getByText("Error")).toBeInTheDocument()
    expect(screen.getByText("New connection…")).toBeInTheDocument()
  })

  it("keeps the database navigation usable after a successful reconnect closes automatically", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "pg-uuid",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null
    })
    mockProfileOpen.mockRejectedValueOnce({
      code: "credentialRequired",
      message: "credential required"
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))
    fireEvent.change(await screen.findByLabelText("Password"), { target: { value: "valid-password" } })
    fireEvent.click(screen.getByText("Connect"))

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(screen.getByText("d@h")).toBeInTheDocument()
    expect(screen.getByText("Connected")).toBeInTheDocument()
    expect(screen.getByText("New connection…")).toBeInTheDocument()
    expect(mockProfileUpdate).toHaveBeenCalledWith(expect.objectContaining({
      descriptorId: "pg-uuid",
      replacementCredential: { password: "valid-password" }
    }))
    expect(mockProfileCreate).not.toHaveBeenCalled()
  })

  it("does not let a dismissed reconnect close a later dialog when its request resolves", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "pg-uuid",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null
    })
    let resolveOpen!: (result: DbLiveConnection) => void
    mockProfileOpen.mockRejectedValueOnce({
      code: "credentialRequired",
      message: "credential required"
    })
    mockProfileOpen.mockImplementationOnce(() => new Promise((resolve) => { resolveOpen = resolve }))
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))
    fireEvent.change(await screen.findByLabelText("Password"), { target: { value: "valid-password" } })
    fireEvent.click(screen.getByText("Connect"))
    await waitFor(() => expect(mockProfileOpen).toHaveBeenCalledTimes(2))

    fireEvent.click(screen.getByText("Cancel"))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    fireEvent.click(screen.getByText("New connection…"))
    expect(screen.getByRole("dialog")).toBeInTheDocument()
    expect(screen.getByText("New connection")).toBeInTheDocument()

    await act(async () => {
      resolveOpen({
        descriptorId: "pg-uuid" as never,
        connectionId: "db-late" as never,
        connectionGeneration: "generation-late" as never,
        engine: "postgres"
      })
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(screen.getByRole("dialog")).toBeInTheDocument()
    expect(screen.getByText("New connection")).toBeInTheDocument()
  })

  it("the New connection dialog offers all three engines and reveals network fields", () => {
    // No saved rows, so the only "SQLite" label on screen is the engine choice.
    useDbStore.setState({ connections: [], saved: [], activeConnId: null })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("New connection…"))
    expect(screen.getByText("New connection")).toBeInTheDocument()
    expect(screen.getByText("SQLite")).toBeInTheDocument()
    expect(screen.getByText("PostgreSQL")).toBeInTheDocument()
    expect(screen.getByText("MSSQL")).toBeInTheDocument()

    fireEvent.click(screen.getByText("PostgreSQL"))
    expect(screen.getByText("Host")).toBeInTheDocument()
    expect(screen.getByText("Database")).toBeInTheDocument()
    expect(screen.getByText("Password")).toBeInTheDocument()
    expect(screen.getByText("Use SSL")).toBeInTheDocument()
  })

  it("treats a cancelled SQLite picker as a no-op but surfaces a rejected picker safely", async () => {
    useDbStore.setState({ connections: [], saved: [], activeConnId: null })
    render(<DatabaseNavContent />)
    fireEvent.click(screen.getByText("New connection…"))

    fireEvent.click(screen.getByText("Browse…"))
    await waitFor(() => expect(mockOpenFileDialog).toHaveBeenCalledTimes(1))
    expect(screen.queryByText("The database operation could not be completed.")).not.toBeInTheDocument()

    mockOpenFileDialog.mockRejectedValueOnce(new Error("RAW PICKER DETAIL"))
    fireEvent.click(screen.getByText("Browse…"))
    expect(await screen.findByText("The database operation could not be completed.")).toBeInTheDocument()
    expect(screen.queryByText("RAW PICKER DETAIL")).not.toBeInTheDocument()
  })

  it("Test connection uses an ephemeral credential without saving, opening, or persisting it", async () => {
    const secret = "YUZORA_TEST_ONLY_SECRET"
    useDbStore.setState({ connections: [], saved: [], activeConnId: null })
    render(<DatabaseNavContent />)
    await fillNewPostgresForm(secret)

    fireEvent.click(screen.getByText("Test connection"))

    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledWith({
      kind: "ephemeral",
      target: {
        kind: "postgres",
        host: "db.example",
        port: 5432,
        database: "app",
        user: "alice",
        ssl: false,
        trustCert: false
      },
      credential: { password: secret }
    }))
    expect(mockProfileCreate).not.toHaveBeenCalled()
    expect(mockProfileUpdate).not.toHaveBeenCalled()
    expect(mockProfileOpen).not.toHaveBeenCalled()
    expect(useDbStore.getState().saved).toEqual([])
    expect(JSON.stringify(useDbStore.getState())).not.toContain(secret)
    const persisted = Array.from({ length: localStorage.length }, (_, index) => {
      const key = localStorage.key(index)
      return key ? localStorage.getItem(key) : null
    }).join("\n")
    expect(persisted).not.toContain(secret)
  })

  it("disables edit Test Connection with localized guidance when a network draft changed but password is empty", async () => {
    const profile = storedPostgres("pg-edit-disabled")
    useDbStore.setState({ connections: [], saved: [profile], activeConnId: null })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    fireEvent.change(screen.getByDisplayValue("h"), { target: { value: "new.host" } })

    const testButton = screen.getByRole("button", { name: "Test connection" })
    expect(testButton).toBeDisabled()
    expect(screen.getByText("Enter the password to test the edited connection settings.")).toBeInTheDocument()
    await act(async () => { await i18n.changeLanguage("zh-TW") })
    expect(screen.getByText("請輸入密碼，才能測試修改後的連線設定。")).toBeInTheDocument()
    fireEvent.click(testButton)
    expect(mockTestConnection).not.toHaveBeenCalled()
  })

  it("tests an unchanged edit through the saved descriptor without reading its credential", async () => {
    const profile = storedPostgres("pg-edit-saved")
    useDbStore.setState({ connections: [], saved: [profile], activeConnId: null })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))

    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledWith({
      kind: "saved",
      descriptorId: "pg-edit-saved"
    }))
    expect(mockTestConnection).toHaveBeenCalledTimes(1)
  })

  it("tests a changed edit as an ephemeral draft when a new password is supplied", async () => {
    const profile = storedPostgres("pg-edit-ephemeral")
    useDbStore.setState({ connections: [], saved: [profile], activeConnId: null })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    fireEvent.change(screen.getByDisplayValue("h"), { target: { value: "new.host" } })
    fireEvent.click(screen.getByRole("radio", { name: /Replace credential/ }))
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "draft-secret" } })
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))

    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledWith({
      kind: "ephemeral",
      target: {
        kind: "postgres",
        host: "new.host",
        port: 5432,
        database: "d",
        user: "u",
        ssl: false,
        trustCert: false
      },
      credential: { password: "draft-secret" }
    }))
    expect(mockProfileUpdate).not.toHaveBeenCalled()
    expect(mockProfileCreate).not.toHaveBeenCalled()
  })

  it("tests a changed SQLite path as an ephemeral draft without requiring a password", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{
        id: "sqlite-edit",
        targetKey: "/tmp/old.db",
        kind: "sqlite",
        name: "old.db",
        path: "/tmp/old.db",
        credentialState: "notRequired"
      }],
      activeConnId: null
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit old.db"))
    fireEvent.change(screen.getByDisplayValue("/tmp/old.db"), {
      target: { value: "/tmp/new.db" }
    })
    const testButton = screen.getByRole("button", { name: "Test connection" })
    expect(testButton).toBeEnabled()
    expect(screen.queryByText("Enter the password to test the edited connection settings.")).not.toBeInTheDocument()
    fireEvent.click(testButton)

    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledWith({
      kind: "ephemeral",
      target: { kind: "sqlite", path: "/tmp/new.db" },
      credential: null
    }))
  })

  it("keeps Test and Save and Connect mutually disabled while either request owns the secret", async () => {
    const descriptorId = "profile-mutual" as DbProfileDescriptor["descriptorId"]
    let resolveTest!: (value: Awaited<ReturnType<typeof dbTestConnection>>) => void
    let resolveCreate!: (value: Awaited<ReturnType<typeof dbProfileCreate>>) => void
    mockTestConnection.mockImplementationOnce(() => new Promise((resolve) => { resolveTest = resolve }))
    useDbStore.setState({ connections: [], saved: [], activeConnId: null })
    render(<DatabaseNavContent />)
    await fillNewPostgresForm("one-owner-only")

    fireEvent.click(screen.getByText("Test connection"))
    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledTimes(1))
    expect(screen.getByText("Save and Connect")).toBeDisabled()
    expect(screen.getByText("Testing…")).toBeDisabled()

    await act(async () => resolveTest({ elapsedMs: 4, serverVersion: "test-db" }))
    await waitFor(() => expect(screen.getByText("Save and Connect")).toBeEnabled())

    mockProfileCreate.mockImplementationOnce(() => new Promise((resolve) => { resolveCreate = resolve }))
    fireEvent.click(screen.getByText("Save and Connect"))
    await waitFor(() => expect(mockProfileCreate).toHaveBeenCalledTimes(1))
    expect(screen.getByText("Test connection")).toBeDisabled()
    expect(mockTestConnection).toHaveBeenCalledTimes(1)

    await act(async () => resolveCreate({
      outcome: "connected",
      profile: {
        descriptorId,
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
      },
      connection: {
        descriptorId,
        connectionId: "connection-mutual" as never,
        connectionGeneration: "generation-mutual" as never,
        engine: "postgres"
      }
    }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  })

  it("retains a saved profile and shows explicit saved-but-connect-failed copy", async () => {
    const descriptorId = "profile-saved" as DbProfileDescriptor["descriptorId"]
    mockProfileCreate.mockResolvedValueOnce({
      outcome: "savedButConnectFailed",
      profile: {
        descriptorId,
        configGeneration: 1,
        name: "failed.db",
        target: { kind: "sqlite", path: "/tmp/failed.db" },
        credentialState: "notRequired"
      },
      error: { code: "connectionFailed", message: "RAW DRIVER DETAIL" }
    })
    useDbStore.setState({ connections: [], saved: [], activeConnId: null })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("New connection…"))
    fireEvent.change(screen.getByPlaceholderText("/path/to/database.sqlite"), {
      target: { value: "/tmp/failed.db" }
    })
    fireEvent.click(screen.getByText("Save and Connect"))

    expect(await screen.findByText("The connection profile was saved, but the connection failed.")).toBeInTheDocument()
    expect(screen.queryByText("RAW DRIVER DETAIL")).not.toBeInTheDocument()
    expect(useDbStore.getState().saved.map((profile) => profile.id)).toEqual([descriptorId])
    expect(useDbStore.getState().connections).toEqual([])
  })

  it("localizes a vault error in English and Traditional Chinese without exposing backend detail", async () => {
    mockProfileList.mockRejectedValue({
      code: "vaultDenied",
      message: "RAW KEYRING DETAIL"
    })
    render(<DatabaseNavContent />)

    expect(await screen.findByText("The operating system denied access to the saved credential.")).toBeInTheDocument()
    expect(screen.queryByText("RAW KEYRING DETAIL")).not.toBeInTheDocument()

    await act(async () => { await i18n.changeLanguage("zh-TW") })
    expect(await screen.findByText("作業系統拒絕存取已儲存的憑證。")).toBeInTheDocument()
    expect(screen.queryByText("RAW KEYRING DETAIL")).not.toBeInTheDocument()
  })
})

describe("DatabaseNavContent status + edit + delete (SSH parity)", () => {
  it("renders the session status badge (connected, then error with an error tooltip)", () => {
    seed({
      connections: [sqliteConn("db-1", "/a.db")],
      activeConnId: "db-1"
    })
    useDbStore.setState({
      sessions: { "/a.db": { descriptorId: "/a.db", connId: "db-1", status: "connected", error: null } }
    })
    const { unmount } = render(<DatabaseNavContent />)
    expect(screen.getByText("Connected")).toBeInTheDocument()
    unmount()

    useDbStore.setState({
      sessions: {
        "/a.db": {
          descriptorId: "/a.db",
          connId: null,
          status: "error",
          error: "connectionFailed"
        }
      }
    })
    render(<DatabaseNavContent />)
    const badge = screen.getByText("Error")
    expect(badge).toHaveAttribute("title", "The database connection failed.")
  })

  it("Pencil opens an editable dialog; Save updates the descriptor in place with the same id", async () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "pg-uuid",
          targetKey: "postgres:h:5432:d",
          kind: "postgres",
          name: "d@h",
          host: "h",
          port: 5432,
          database: "d",
          user: "u",
          ssl: false
        }
      ],
      activeConnId: null,
      sessions: {}
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    // Edit mode: the edit-variant title, and connection fields are editable.
    expect(screen.getByText("Edit connection")).toBeInTheDocument()
    expect(screen.getByRole("radio", { name: /Keep credential/ })).toBeChecked()
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument()
    const hostInput = screen.getByDisplayValue("h")
    expect(hostInput).not.toHaveAttribute("readonly")

    fireEvent.change(hostInput, { target: { value: "newhost" } })
    fireEvent.click(screen.getByText("Save"))

    await waitFor(() => expect(useDbStore.getState().saved[0]?.host).toBe("newhost"))
    const saved = useDbStore.getState().saved[0]
    expect(saved.id).toBe("pg-uuid")
    expect(saved.host).toBe("newhost")
    expect(saved.targetKey).toBe("postgres:newhost:5432:d")
    expect(screen.queryByText("Edit connection")).not.toBeInTheDocument()
    expect(screen.getByText("d@newhost")).toBeInTheDocument()
    expect(mockProfileUpdate).toHaveBeenCalledWith(expect.objectContaining({
      descriptorId: "pg-uuid",
      replacementCredential: null
    }))
    expect(mockProfileCreate).not.toHaveBeenCalled()
  })

  it("Replace Credential requires a new password and sends only that write-only replacement", async () => {
    const profile = storedPostgres("pg-replace")
    useDbStore.setState({ connections: [], saved: [profile], activeConnId: null, sessions: {} })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    fireEvent.click(screen.getByRole("radio", { name: /Replace credential/ }))
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
    const password = screen.getByLabelText("Password")
    expect(password).toHaveValue("")
    fireEvent.change(password, { target: { value: "replacement-only-secret" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(mockProfileUpdate).toHaveBeenCalledWith(expect.objectContaining({
      descriptorId: "pg-replace",
      replacementCredential: { password: "replacement-only-secret" }
    })))
    expect(JSON.stringify(useDbStore.getState())).not.toContain("replacement-only-secret")
    expect(JSON.stringify(localStorage)).not.toContain("replacement-only-secret")
  })

  it("Remove Credential edits the descriptor first and keeps a localized failure visible", async () => {
    const profile = storedPostgres("pg-edit-remove")
    useDbStore.setState({ connections: [], saved: [profile], activeConnId: null, sessions: {} })
    mockProfileRemoveCredential.mockRejectedValueOnce({
      code: "vaultDeleteFailed",
      message: "RAW VAULT DELETE DETAIL"
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Edit d@h"))
    fireEvent.change(screen.getByDisplayValue("h"), { target: { value: "edited.example" } })
    fireEvent.click(screen.getByRole("radio", { name: /Remove credential/ }))
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(await screen.findByText(
      "The credential could not be removed from the operating system vault."
    )).toBeInTheDocument()
    expect(screen.getByRole("dialog")).toBeInTheDocument()
    expect(screen.queryByText("RAW VAULT DELETE DETAIL")).not.toBeInTheDocument()
    expect(mockProfileUpdate).toHaveBeenCalledWith(expect.objectContaining({
      descriptorId: "pg-edit-remove",
      target: expect.objectContaining({ host: "edited.example" }),
      replacementCredential: null
    }))
    expect(mockProfileRemoveCredential).toHaveBeenCalledWith("pg-edit-remove")
    expect(mockProfileUpdate.mock.invocationCallOrder[0]).toBeLessThan(
      mockProfileRemoveCredential.mock.invocationCallOrder[0]
    )
  })

  it("two-step delete: Trash → cancel keeps it; Trash → confirm removes it", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{ id: "/x.db", targetKey: "/x.db", kind: "sqlite", name: "x.db", path: "/x.db" }],
      activeConnId: null,
      sessions: {}
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Forget x.db"))
    fireEvent.click(screen.getByLabelText("Keep connection"))
    expect(useDbStore.getState().saved).toHaveLength(1)

    fireEvent.click(screen.getByLabelText("Forget x.db"))
    fireEvent.click(screen.getByLabelText("Confirm removing x.db"))
    await waitFor(() => expect(useDbStore.getState().saved).toEqual([]))
  })

  it("Remove Credential keeps the profile but marks its credential required", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{
        id: "pg-remove-credential",
        targetKey: "postgres:h:5432:d",
        kind: "postgres",
        name: "d@h",
        host: "h",
        port: 5432,
        database: "d",
        user: "u",
        ssl: false,
        credentialState: "stored"
      }],
      activeConnId: null,
      sessions: {}
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByLabelText("Remove saved credential for d@h"))

    await waitFor(() => expect(mockProfileRemoveCredential).toHaveBeenCalledWith("pg-remove-credential"))
    expect(useDbStore.getState().saved).toHaveLength(1)
    expect(useDbStore.getState().saved[0].credentialState).toBe("required")
  })

  it("renders a startup recovery row and runs only its explicit recovery action", async () => {
    mockProfileList.mockResolvedValueOnce({
      profiles: [],
      recovery: [{
        operationId: "op-cleanup",
        descriptorId: "profile-cleanup" as DbProfileDescriptor["descriptorId"],
        kind: "cleanupOld",
        allowedActions: ["retryCleanup"]
      }]
    })
    mockProfileRecover.mockResolvedValueOnce({ profiles: [], recovery: [] })
    render(<DatabaseNavContent />)

    fireEvent.click(await screen.findByText("Retry cleanup"))

    await waitFor(() => expect(mockProfileRecover).toHaveBeenCalledWith({
      operationId: "op-cleanup",
      action: "retryCleanup",
      credential: null
    }))
    expect(screen.queryByText("Retry cleanup")).not.toBeInTheDocument()
  })

  it("Resume completes with an existing vault generation without prompting", async () => {
    const recovery = [{
      operationId: "op-resume-existing",
      descriptorId: "profile-existing" as DbProfileDescriptor["descriptorId"],
      kind: "pendingCreate" as const,
      allowedActions: ["resume" as const]
    }]
    mockProfileList.mockResolvedValue({ profiles: [], recovery })
    mockProfileRecover.mockResolvedValueOnce({ profiles: [], recovery: [] })
    render(<DatabaseNavContent />)

    fireEvent.click(await screen.findByText("Resume"))

    await waitFor(() => expect(mockProfileRecover).toHaveBeenCalledWith({
      operationId: "op-resume-existing",
      action: "resume",
      credential: null
    }))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(mockProfileRecover).toHaveBeenCalledTimes(1)
  })

  it("Resume opens exactly one credential prompt only after a missing generation response", async () => {
    const recovery = [{
      operationId: "op-resume-missing",
      descriptorId: "profile-missing" as DbProfileDescriptor["descriptorId"],
      kind: "pendingReplace" as const,
      allowedActions: ["resume" as const]
    }]
    mockProfileList.mockResolvedValue({ profiles: [], recovery })
    mockProfileRecover
      .mockRejectedValueOnce({ code: "credentialRequired", message: "RAW VAULT DETAIL" })
      .mockResolvedValueOnce({ profiles: [], recovery: [] })
    render(<DatabaseNavContent />)

    fireEvent.click(await screen.findByText("Resume"))

    const dialog = await screen.findByRole("dialog")
    expect(screen.getAllByRole("dialog")).toHaveLength(1)
    expect(mockProfileRecover).toHaveBeenCalledTimes(1)
    expect(mockProfileRecover).toHaveBeenNthCalledWith(1, {
      operationId: "op-resume-missing",
      action: "resume",
      credential: null
    })
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: "replacement-secret" }
    })
    fireEvent.click(within(dialog).getByText("Resume"))

    await waitFor(() => expect(mockProfileRecover).toHaveBeenCalledTimes(2))
    expect(mockProfileRecover).toHaveBeenNthCalledWith(2, {
      operationId: "op-resume-missing",
      action: "resume",
      credential: { password: "replacement-secret" }
    })
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(screen.queryByText("RAW VAULT DETAIL")).not.toBeInTheDocument()
  })

  it("Resume does not prompt for a non-recoverable vault error", async () => {
    const recovery = [{
      operationId: "op-resume-unavailable",
      descriptorId: "profile-unavailable" as DbProfileDescriptor["descriptorId"],
      kind: "pendingCreate" as const,
      allowedActions: ["resume" as const]
    }]
    mockProfileList.mockResolvedValue({ profiles: [], recovery })
    mockProfileRecover.mockRejectedValueOnce({
      code: "vaultUnavailable",
      message: "RAW VAULT DETAIL"
    })
    render(<DatabaseNavContent />)

    fireEvent.click(await screen.findByText("Resume"))

    expect(await screen.findByText("The operating system credential vault is unavailable.")).toBeInTheDocument()
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(mockProfileRecover).toHaveBeenCalledTimes(1)
    expect(screen.queryByText("RAW VAULT DETAIL")).not.toBeInTheDocument()
  })

  it("Abort dispatches an explicit credential-free abort transition", async () => {
    mockProfileList.mockResolvedValueOnce({
      profiles: [],
      recovery: [{
        operationId: "op-abort",
        descriptorId: "profile-abort" as DbProfileDescriptor["descriptorId"],
        kind: "pendingCreate",
        allowedActions: ["abort"]
      }]
    })
    mockProfileRecover.mockResolvedValueOnce({ profiles: [], recovery: [] })
    render(<DatabaseNavContent />)

    fireEvent.click(await screen.findByText("Abort"))

    await waitFor(() => expect(mockProfileRecover).toHaveBeenCalledWith({
      operationId: "op-abort",
      action: "abort",
      credential: null
    }))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })
})
