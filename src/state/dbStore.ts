import { create } from "zustand"

import { dbClose, dbListTables, dbOpen, dbQuery } from "@/lib/ipc"
import type { DbKind, DbOpenConfig, DbQueryResult, DbTable } from "@/lib/types"

/** A live connection. `key` is the stable per-database identity (file path for
 *  SQLite, `kind:host:port:database` for network DBs) used for query history and
 *  saved-descriptor lookup — connId is reassigned on every reopen. */
export interface DbConnection {
    connId: string
    kind: DbKind
    /** Basename / `database@host` shown in the connection list. */
    name: string
    key: string
    /** Full path or `user@host:port/database` for the row tooltip. */
    title: string
}

/** A persisted, non-secret connection descriptor (Q5). The password is NEVER
 *  stored — network reconnects prompt for it. `id` equals the connection `key`. */
export interface SavedDbConnection {
    id: string
    kind: DbKind
    name: string
    /** SQLite only. */
    path?: string
    /** Network only. */
    host?: string
    port?: number
    database?: string
    user?: string
    /** PostgreSQL TLS. */
    ssl?: boolean
    /** MSSQL trust self-signed certificate. */
    trustCert?: boolean
}

/** Active column sort, pushed down to SQL. `columnIndex` is the *original* column
 *  index (not the dragged display position). */
export interface DbSort {
    columnIndex: number
    dir: "asc" | "desc"
}

export interface DbQueryState {
    sql: string
    running: boolean
    result: DbQueryResult | null
    error: string | null
    elapsedMs: number | null
    /** SQL that produced the current select result — the base a header-sort wraps
     *  as a subquery. Null unless the last run was a successful select. */
    lastSql: string | null
    sortBy: DbSort | null
}

/** One recorded SQL run, kept per database so the console remembers past queries
 *  across sessions. `ranAt` is `Date.now()` (ms); `error` is only set when `ok`
 *  is false. */
export interface DbHistoryEntry {
    sql: string
    ranAt: number
    ok: boolean
    error?: string
    elapsedMs: number
}

// Persisted query history. Keyed by the connection `key` (path for SQLite,
// kind:host:port:database for network DBs) so it bridges reopen/restart.
export const DB_HISTORY_STORAGE_KEY = "yuzora.db.history.v1"
// Persisted saved connection descriptors (no secrets).
export const DB_CONNECTIONS_STORAGE_KEY = "yuzora.db.connections.v1"
/** Newest-kept entries per key. */
export const DB_HISTORY_LIMIT = 50
/** Defensive per-entry cap so a pathological paste can't bloat localStorage. */
export const DB_HISTORY_SQL_MAX = 5000

interface DbState {
    connections: DbConnection[]
    activeConnId: string | null
    /** Persisted descriptors, connected or not. */
    saved: SavedDbConnection[]
    /** Tables keyed by connId. */
    tables: Record<string, DbTable[]>
    /**
     * SQL buffer + last result, keyed by connId. A query always resolves into
     * the bucket for the connId it was run against — never into whichever
     * connection happens to be active by the time it completes — so switching
     * the active connection mid-query can't leak rows/errors onto the wrong
     * connection, and `running` always settles correctly even if the user
     * switched away and back before the query finished.
     */
    queries: Record<string, DbQueryState>
    /** Past query runs keyed by connection `key` (survives reopen/restart). */
    history: Record<string, DbHistoryEntry[]>
    /** Open (or focus) a database from a tagged descriptor; persists the
     *  non-secret descriptor for reconnect. */
    openConfig: (config: DbOpenConfig) => Promise<void>
    /** SQLite convenience wrapper over openConfig. */
    openConnection: (path: string) => Promise<void>
    closeConnection: (connId: string) => Promise<void>
    /** Forget a saved descriptor (and close its live connection, if any). */
    removeSaved: (id: string) => void
    setActiveConnection: (connId: string) => void
    loadTables: (connId: string) => Promise<void>
    setSql: (sql: string) => void
    runQuery: () => Promise<void>
    /** Re-run the last select wrapped in an ORDER BY, cycling the clicked column
     *  asc → desc → cleared. `columnIndex` is the original column index. */
    sortResult: (columnIndex: number) => Promise<void>
    openTableQuery: (table: string) => Promise<void>
    /** Append a run to a connection key's history (truncates sql, skips a
     *  consecutive duplicate, caps the list, persists). */
    recordHistory: (key: string, entry: DbHistoryEntry) => void
    reset: () => void
}

function basename(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/** Double-quote a SQLite/PostgreSQL identifier so a click-to-query stays
 *  injection-safe. */
function quoteIdent(name: string): string {
    return `"${name.replace(/"/g, '""')}"`
}

/** Bracket-quote an MSSQL identifier (`]` escaped as `]]`). */
function quoteIdentMssql(name: string): string {
    return `[${name.replace(/]/g, "]]")}]`
}

/** Stable per-database key: file path (SQLite) or kind:host:port:database. */
function configKey(config: DbOpenConfig): string {
    return config.kind === "sqlite"
        ? config.path
        : `${config.kind}:${config.host}:${config.port}:${config.database}`
}

/** Project a descriptor out of an open config, dropping the password. */
function savedFromConfig(config: DbOpenConfig): SavedDbConnection {
    if (config.kind === "sqlite") {
        return { id: config.path, kind: "sqlite", name: basename(config.path), path: config.path }
    }
    const base: SavedDbConnection = {
        id: configKey(config),
        kind: config.kind,
        name: `${config.database}@${config.host}`,
        host: config.host,
        port: config.port,
        database: config.database,
        user: config.user
    }
    return config.kind === "postgres"
        ? { ...base, ssl: config.ssl }
        : { ...base, trustCert: config.trustCert }
}

function savedTitle(s: SavedDbConnection): string {
    return s.kind === "sqlite"
        ? (s.path ?? s.name)
        : `${s.user ?? ""}@${s.host ?? ""}:${s.port ?? ""}/${s.database ?? ""}`
}

function connectionFromSaved(connId: string, s: SavedDbConnection): DbConnection {
    return { connId, kind: s.kind, name: s.name, key: s.id, title: savedTitle(s) }
}

// Re-project through the field whitelist so nothing outside the descriptor (in
// particular any secret) can survive a load/save round-trip.
function sanitizeSaved(s: SavedDbConnection): SavedDbConnection {
    const out: SavedDbConnection = { id: s.id, kind: s.kind, name: s.name }
    if (s.kind === "sqlite") {
        if (s.path) out.path = s.path
        return out
    }
    out.host = s.host
    out.port = s.port
    out.database = s.database
    out.user = s.user
    if (s.kind === "postgres") out.ssl = s.ssl
    if (s.kind === "mssql") out.trustCert = s.trustCert
    return out
}

function isSavedConnection(value: unknown): value is SavedDbConnection {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    if (typeof v.id !== "string" || typeof v.name !== "string") return false
    if (v.kind === "sqlite") return typeof v.path === "string"
    if (v.kind === "postgres" || v.kind === "mssql") {
        return (
            typeof v.host === "string" &&
            typeof v.port === "number" &&
            typeof v.database === "string" &&
            typeof v.user === "string"
        )
    }
    return false
}

export function loadSavedConnections(): SavedDbConnection[] {
    try {
        const raw = localStorage.getItem(DB_CONNECTIONS_STORAGE_KEY)
        if (!raw) return []
        const parsed = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        return parsed.filter(isSavedConnection).map(sanitizeSaved)
    } catch {
        return []
    }
}

function saveConnections(list: SavedDbConnection[]): void {
    try {
        localStorage.setItem(DB_CONNECTIONS_STORAGE_KEY, JSON.stringify(list.map(sanitizeSaved)))
    } catch {
        // private mode / quota — the in-memory list stays authoritative
    }
}

/** Strip a trailing `;` and a trailing `-- …` line comment so the SQL can be
 *  wrapped as a subquery. `SELECT 1;` or `SELECT 1 -- note` would otherwise make
 *  `SELECT * FROM (…) ORDER BY 1` a parse error. */
export function stripTrailingSql(sql: string): string {
    let s = sql
    for (;;) {
        const before = s
        s = s.replace(/\s+$/, "")
        s = s.replace(/;+$/, "")
        s = s.replace(/--[^\n]*$/, "")
        if (s === before) return s
    }
}

/** Wrap a base select as a subquery ordered by a 1-based column ordinal. The
 *  newline before the closing paren keeps a trailing line comment inside the base
 *  from swallowing it. MSSQL requires the derived table to be aliased (`_yz`);
 *  the outer ORDER BY by ordinal is portable across all three engines. */
export function buildSortedSql(
    kind: DbKind,
    baseSql: string,
    ordinal: number,
    dir: "asc" | "desc"
): string {
    const d = dir === "asc" ? "ASC" : "DESC"
    const base = stripTrailingSql(baseSql)
    const alias = kind === "mssql" ? " AS _yz" : ""
    return `SELECT * FROM (\n${base}\n)${alias} ORDER BY ${ordinal} ${d}`
}

/** A LIMIT-100 (or TOP 100) preview select for a table, per dialect. */
export function buildTableQuery(kind: DbKind, table: string): string {
    if (kind === "mssql") return `SELECT TOP 100 * FROM ${quoteIdentMssql(table)}`
    return `SELECT * FROM ${quoteIdent(table)} LIMIT 100`
}

/** Stable empty-state reference: the default for a connId with no query
 *  bucket yet, so selectors reading it don't allocate a new object per call. */
const EMPTY_DB_QUERY: DbQueryState = {
    sql: "",
    running: false,
    result: null,
    error: null,
    elapsedMs: null,
    lastSql: null,
    sortBy: null
}

/** Resolve a connId's query bucket, falling back to EMPTY_DB_QUERY. */
export function queryFor(
    s: { queries: Record<string, DbQueryState> },
    connId: string | null
): DbQueryState {
    return (connId && s.queries[connId]) || EMPTY_DB_QUERY
}

function isHistoryEntry(value: unknown): value is DbHistoryEntry {
    if (typeof value !== "object" || value === null) return false
    const e = value as Record<string, unknown>
    return (
        typeof e.sql === "string" &&
        typeof e.ranAt === "number" &&
        typeof e.ok === "boolean" &&
        typeof e.elapsedMs === "number" &&
        (e.error === undefined || typeof e.error === "string")
    )
}

export function loadHistory(): Record<string, DbHistoryEntry[]> {
    try {
        const raw = localStorage.getItem(DB_HISTORY_STORAGE_KEY)
        if (!raw) return {}
        const parsed = JSON.parse(raw)
        if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {}
        const out: Record<string, DbHistoryEntry[]> = {}
        for (const [key, entries] of Object.entries(parsed)) {
            if (Array.isArray(entries)) {
                const kept = entries.filter(isHistoryEntry).slice(0, DB_HISTORY_LIMIT)
                if (kept.length > 0) out[key] = kept
            }
        }
        return out
    } catch {
        return {}
    }
}

function saveHistory(history: Record<string, DbHistoryEntry[]>): void {
    try {
        localStorage.setItem(DB_HISTORY_STORAGE_KEY, JSON.stringify(history))
    } catch {
        // private mode / quota — the in-memory history stays authoritative
    }
}

export const dbInitialState = {
    connections: [] as DbConnection[],
    activeConnId: null as string | null,
    tables: {} as Record<string, DbTable[]>,
    queries: {} as Record<string, DbQueryState>
}

// Guards a double-click / double-submit from opening two live connections to the
// same descriptor while the first dbOpen IPC is still in flight (the existing()
// dedup below only sees connections that already resolved).
const openingKeys = new Set<string>()

export const useDbStore = create<DbState>()((set, get) => ({
    ...dbInitialState,
    history: loadHistory(),
    saved: loadSavedConnections(),

    openConfig: async (config) => {
        const descriptor = savedFromConfig(config)
        // Persist the descriptor (dedup by id) so it survives restart.
        set((s) => {
            const next = [...s.saved.filter((x) => x.id !== descriptor.id), descriptor]
            saveConnections(next)
            return { saved: next }
        })
        // Already open? Just focus it (avoids a duplicate live connection).
        const existing = get().connections.find((c) => c.key === descriptor.id)
        if (existing) {
            get().setActiveConnection(existing.connId)
            return
        }
        // In-flight for this descriptor? Drop the duplicate request.
        if (openingKeys.has(descriptor.id)) return
        openingKeys.add(descriptor.id)
        try {
            const { connId } = await dbOpen(config)
            const conn = connectionFromSaved(connId, descriptor)
            set((s) => ({ connections: [...s.connections, conn], activeConnId: connId }))
            await get().loadTables(connId)
        } finally {
            openingKeys.delete(descriptor.id)
        }
    },

    openConnection: (path) => get().openConfig({ kind: "sqlite", path }),

    closeConnection: async (connId) => {
        try {
            await dbClose(connId)
        } catch {
            // Best-effort: drop the connection from the UI even if the backend
            // close errored (the registry entry is gone either way on next open).
        }
        set((s) => {
            const connections = s.connections.filter((c) => c.connId !== connId)
            const tables = { ...s.tables }
            delete tables[connId]
            const queries = { ...s.queries }
            delete queries[connId]
            const wasActive = s.activeConnId === connId
            return {
                connections,
                tables,
                queries,
                activeConnId: wasActive ? (connections[0]?.connId ?? null) : s.activeConnId
            }
        })
    },

    removeSaved: (id) => {
        const live = get().connections.find((c) => c.key === id)
        if (live) void get().closeConnection(live.connId)
        set((s) => {
            const next = s.saved.filter((x) => x.id !== id)
            saveConnections(next)
            return { saved: next }
        })
    },

    setActiveConnection: (connId) => {
        const s = get()
        if (s.activeConnId === connId) return
        if (!s.connections.some((c) => c.connId === connId)) return
        set({ activeConnId: connId })
        if (!get().tables[connId]) void get().loadTables(connId)
    },

    loadTables: async (connId) => {
        try {
            const tables = await dbListTables(connId)
            set((s) => ({ tables: { ...s.tables, [connId]: tables } }))
        } catch {
            // A db that opens but fails to enumerate tables shows an empty list
            // rather than surfacing a nav-level error.
            set((s) => ({ tables: { ...s.tables, [connId]: [] } }))
        }
    },

    setSql: (sql) =>
        set((s) => {
            const connId = s.activeConnId
            if (!connId) return {}
            return { queries: { ...s.queries, [connId]: { ...queryFor(s, connId), sql } } }
        }),

    runQuery: async () => {
        const s = get()
        const connId = s.activeConnId
        if (!connId) return
        const current = queryFor(s, connId)
        if (current.running) return
        const sql = current.sql.trim()
        if (!sql) return
        // The history key of the connection this query runs against — captured now
        // so history lands on the right database even if the user switches
        // connections before the query resolves.
        const key = s.connections.find((c) => c.connId === connId)?.key ?? null
        set((cur) => ({
            queries: { ...cur.queries, [connId]: { ...queryFor(cur, connId), running: true, error: null } }
        }))
        const started = performance.now()
        try {
            const result = await dbQuery(connId, sql)
            const elapsedMs = Math.round(performance.now() - started)
            // Always settles into connId's own bucket — correct regardless of
            // whichever connection is active by the time this resolves.
            set((cur) => ({
                queries: {
                    ...cur.queries,
                    [connId]: {
                        ...queryFor(cur, connId),
                        running: false,
                        result,
                        error: null,
                        elapsedMs,
                        // A fresh run becomes the base for header sorting and clears
                        // any prior sort; only a select is sortable.
                        lastSql: result.kind === "select" ? sql : null,
                        sortBy: null
                    }
                }
            }))
            if (key) get().recordHistory(key, { sql, ranAt: Date.now(), ok: true, elapsedMs })
            // CREATE/DROP/ALTER change the table list — refresh it after a write.
            if (result.kind === "execute") void get().loadTables(connId)
        } catch (e) {
            const elapsedMs = Math.round(performance.now() - started)
            set((cur) => ({
                queries: {
                    ...cur.queries,
                    [connId]: {
                        ...queryFor(cur, connId),
                        running: false,
                        result: null,
                        error: String(e),
                        elapsedMs,
                        lastSql: null,
                        sortBy: null
                    }
                }
            }))
            if (key) get().recordHistory(key, { sql, ranAt: Date.now(), ok: false, error: String(e), elapsedMs })
        }
    },

    sortResult: async (columnIndex) => {
        const s = get()
        const connId = s.activeConnId
        if (!connId) return
        const current = queryFor(s, connId)
        if (current.running) return
        const baseSql = current.lastSql
        if (!baseSql) return
        const kind = s.connections.find((c) => c.connId === connId)?.kind ?? "sqlite"
        // asc → desc → cleared, per clicked column.
        const prev = current.sortBy
        const nextSort: DbSort | null =
            !prev || prev.columnIndex !== columnIndex
                ? { columnIndex, dir: "asc" }
                : prev.dir === "asc"
                  ? { columnIndex, dir: "desc" }
                  : null
        // Cleared → re-run the untouched base; otherwise wrap it (1-based ordinal).
        const sql = nextSort ? buildSortedSql(kind, baseSql, columnIndex + 1, nextSort.dir) : baseSql
        set((cur) => ({
            queries: {
                ...cur.queries,
                [connId]: { ...queryFor(cur, connId), running: true, error: null, sortBy: nextSort }
            }
        }))
        const started = performance.now()
        try {
            const result = await dbQuery(connId, sql)
            const elapsedMs = Math.round(performance.now() - started)
            // lastSql stays the base so a further sort re-wraps the original, not
            // the already-wrapped query.
            set((cur) => ({
                queries: {
                    ...cur.queries,
                    [connId]: { ...queryFor(cur, connId), running: false, result, error: null, elapsedMs }
                }
            }))
        } catch (e) {
            const elapsedMs = Math.round(performance.now() - started)
            set((cur) => ({
                queries: {
                    ...cur.queries,
                    [connId]: { ...queryFor(cur, connId), running: false, result: null, error: String(e), elapsedMs }
                }
            }))
        }
    },

    openTableQuery: async (table) => {
        const connId = get().activeConnId
        if (!connId) return
        const kind = get().connections.find((c) => c.connId === connId)?.kind ?? "sqlite"
        get().setSql(buildTableQuery(kind, table))
        await get().runQuery()
    },

    recordHistory: (key, entry) =>
        set((s) => {
            const sql =
                entry.sql.length > DB_HISTORY_SQL_MAX ? entry.sql.slice(0, DB_HISTORY_SQL_MAX) : entry.sql
            const existing = s.history[key] ?? []
            // A consecutive re-run of the identical statement doesn't earn a new row.
            if (existing[0]?.sql === sql) return {}
            const next = [{ ...entry, sql }, ...existing].slice(0, DB_HISTORY_LIMIT)
            const history = { ...s.history, [key]: next }
            saveHistory(history)
            return { history }
        }),

    reset: () =>
        set({ ...dbInitialState, history: loadHistory(), saved: loadSavedConnections() })
}))
