import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DbConnection, DbHistoryEntry, SavedDbConnection } from "@/state/dbStore"

vi.mock("@/lib/ipc", () => ({
  dbOpen: vi.fn(),
  dbClose: vi.fn(),
  dbListTables: vi.fn(),
  dbQuery: vi.fn()
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))

import { dbOpen, dbListTables, dbQuery } from "@/lib/ipc"
import { useDbStore } from "@/state/dbStore"
import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"

const mockOpen = vi.mocked(dbOpen)
const mockList = vi.mocked(dbListTables)
const mockQuery = vi.mocked(dbQuery)

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

/** A SQLite descriptor for a live connection (id == key == path). */
function sqliteConn(connId: string, path: string): DbConnection {
  const name = path.split("/").pop() ?? path
  return { connId, kind: "sqlite", name, key: path, title: path }
}
function savedFor(conn: DbConnection): SavedDbConnection {
  return { id: conn.key, kind: "sqlite", name: conn.name, path: conn.key }
}

function seed(opts: {
  connections?: DbConnection[]
  saved?: SavedDbConnection[]
  activeConnId?: string | null
  history?: Record<string, DbHistoryEntry[]>
}) {
  const connections = opts.connections ?? [sqliteConn("db-1", "/tmp/app.sqlite")]
  const saved = opts.saved ?? connections.map(savedFor)
  useDbStore.setState({
    connections,
    saved,
    activeConnId: opts.activeConnId ?? connections[0]?.connId ?? null,
    tables: Object.fromEntries(connections.map((c) => [c.connId, []])),
    history: opts.history ?? {}
  })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  useDbStore.getState().reset()
  vi.clearAllMocks()
  mockOpen.mockResolvedValue({ connId: "db-new" })
  mockList.mockResolvedValue([])
})

afterEach(() => {
  cleanup()
})

describe("DatabaseNavContent recent queries", () => {
  it("renders the recent-queries list with sql first line, relative time and a failure dot", () => {
    const now = Date.now()
    seed({
      history: {
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
      history: {
        "/tmp/app.sqlite": [{ sql: "DELETE FROM users", ranAt: Date.now(), ok: true, elapsedMs: 2 }]
      }
    })
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("DELETE FROM users"))

    expect(useDbStore.getState().queries["db-1"].sql).toBe("DELETE FROM users")
    expect(mockQuery).not.toHaveBeenCalled()
  })

  it("shows no recent-queries section when the active connection has no history", () => {
    seed({ history: {} })
    render(<DatabaseNavContent />)
    expect(screen.queryByText("Recent queries")).not.toBeInTheDocument()
  })

  it("only shows history for the active connection's key", () => {
    seed({
      connections: [sqliteConn("db-1", "/a.db"), sqliteConn("db-2", "/b.db")],
      activeConnId: "db-1",
      history: {
        "/a.db": [{ sql: "SELECT 'a'", ranAt: Date.now(), ok: true, elapsedMs: 1 }],
        "/b.db": [{ sql: "SELECT 'b'", ranAt: Date.now(), ok: true, elapsedMs: 1 }]
      }
    })
    render(<DatabaseNavContent />)

    expect(screen.getByText("SELECT 'a'")).toBeInTheDocument()
    expect(screen.queryByText("SELECT 'b'")).not.toBeInTheDocument()
  })
})

describe("DatabaseNavContent saved connections", () => {
  it("shows a saved-but-offline descriptor and clicking it reconnects a SQLite file directly", async () => {
    useDbStore.setState({
      connections: [],
      saved: [{ id: "/x.db", kind: "sqlite", name: "x.db", path: "/x.db" }],
      activeConnId: null
    })
    render(<DatabaseNavContent />)

    expect(screen.getByText("x.db")).toBeInTheDocument()
    expect(screen.getByText(/not connected/)).toBeInTheDocument()

    fireEvent.click(screen.getByText("x.db"))
    // SQLite reconnect opens directly (no dialog / password prompt).
    expect(mockOpen).toHaveBeenCalledWith({ kind: "sqlite", path: "/x.db" })
  })

  it("clicking a saved offline network descriptor opens the reconnect (password) dialog", () => {
    useDbStore.setState({
      connections: [],
      saved: [
        {
          id: "postgres:h:5432:d",
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
    render(<DatabaseNavContent />)

    fireEvent.click(screen.getByText("d@h"))

    expect(screen.getByText("Reconnect")).toBeInTheDocument()
    // Only the password is asked for — the connect button drives openConfig.
    expect(screen.getByText("Connect")).toBeInTheDocument()
    expect(mockOpen).not.toHaveBeenCalled()
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
})
