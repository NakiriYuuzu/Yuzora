import { create } from "zustand"

import {
    buildTableQuery,
    dbObjectRefKey,
    resolveDatabaseSqlTarget
} from "@/lib/databaseSql"
import type {
    DatabaseSqlTargetError,
    DatabaseSqlTargetRequest,
    DatabaseSqlUnit
} from "@/lib/databaseSql"

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
import type {
    DbConnectionIdentity,
    DbConnectionGeneration,
    DbConnectionId,
    DbColumn,
    DbCredentialState,
    DbDescriptorId,
    DbKind,
    DbLiveConnection,
    DbOpenConfig,
    DbOperationalErrorCode,
    DbProfileDescriptor,
    DbProfileErrorCode,
    DbProfileLoadResult,
    DbProfileRecoveryRequest,
    DbProfileRecoveryRow,
    DbSaveAndConnectOutcome,
    DbProfileTarget,
    DbQueryCancelResult,
    DbQueryRun,
    DbQueryRunId,
    DbQueryRunOwner,
    DbQueryResult,
    DbResultPage,
    DbResultSession,
    DbResultSessionOwner,
    DbStatementExecution,
    DbStatementExecutionId,
    DbValue,
    DbError,
    DbTable
} from "@/lib/types"

export type { DbOperationalErrorCode } from "@/lib/types"

/** A live handle projected from one opaque saved descriptor. `targetKey` remains
 * display/legacy-history compatibility data only; descriptorId is the sole
 * profile identity and connId is reassigned on every reopen. */
export interface DbConnection {
    connId: string
    connectionGeneration?: DbConnectionGeneration
    kind: DbKind
    /** Basename / `database@host` shown in the connection list. */
    name: string
    /** Saved descriptor id (opaque UUID) this connection was opened from. */
    descriptorId: string
    /** Content-derived display and one-time legacy-migration key only. */
    targetKey: string
    /** Full path or `user@host:port/database` for the row tooltip. */
    title: string
}

/** A persisted, non-secret connection descriptor (Q5). Credentials live only in
 *  the OS vault; this descriptor and the Zustand/localStorage surfaces never
 *  contain them. `id` is a stable, opaque UUID (unchanged across edits);
 *  `targetKey` is content-derived display/legacy compatibility data only. */
export interface SavedDbConnection {
    id: string
    /** Rust-owned monotonic descriptor revision. Legacy browser records hydrate
     * at generation 1 and are replaced by the repository snapshot on startup. */
    configGeneration?: number
    targetKey: string
    kind: DbKind
    name: string
    credentialState?: DbCredentialState
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

export type DbSessionStatus = "connecting" | "connected" | "error" | "disconnected"

/** Stable UI tokens only. Raw repository/vault/driver messages must not enter
 * React state because they are neither localizable nor safe display contracts. */
export type DbProfileUiErrorCode =
    | DbProfileErrorCode
    | "connectionAuthenticationFailed"
    | "connectionDnsFailed"
    | "connectionTlsFailed"
    | "connectionTimedOut"
    | "connectionServerRejected"
    | "legacyCleanupFailed"
    | "legacyHistoryCleanupFailed"
    | "savedButConnectFailed"
    | "unknown"

/** Stable recovery token plus the backend's optional typed engine diagnostics. */
export interface DbOperationalErrorState {
    code: DbOperationalErrorCode
    databaseError: DbError | null
}

export interface DbQueryErrorState extends DbOperationalErrorState {
    /** Exact SQL sent to the engine; never persisted outside this session. */
    executedSql: string
}

export type DbOperationSlot =
    | "open"
    | "edit"
    | "remove"
    | "disconnect"
    | "tables"
    | "columns"
    | "query"
    | "page"

export type DbOperationCounters = Record<DbOperationSlot, number>

/** Session state keyed by descriptor id, mirroring sshStore. Survives a
 *  disconnect (marked `disconnected`, not removed) so the row keeps its badge. */
export interface DbSessionState {
    descriptorId: string
    connId: string | null
    status: DbSessionStatus
    error: DbProfileUiErrorCode | null
}

export interface DbReconnectRequest {
    descriptorId: string
    /** Monotonic trigger so requesting the same descriptor twice still reopens
     *  a fresh dialog instance. */
    token: number
}

export type DbOpenSavedResult =
    | { outcome: "completed" }
    | { outcome: "cancelled" }
    | { outcome: "error"; error: unknown }

/** Active sort for the currently loaded client page. `columnIndex` is the
 * original column index, never a reason to rewrite or re-run SQL. */
export interface DbSort {
    columnIndex: number
    dir: "asc" | "desc"
}

export interface DbStatementResultPageState {
    page: DbResultPage
    loading: boolean
    pageError: DbOperationalErrorState | null
    released: boolean
    sort: DbSort | null
    /** Unsorted rows from the latest backend page, retained so clearing a sort
     * restores the exact wire order without mutating the QueryRun fixture. */
    sortBaseRows: DbValue[][]
}

export type DbQueryRunGroupStatus = "running" | "cancelling" | "settled"

export interface DbQueryRunGroup {
    owner: DbQueryRunOwner
    mode: "primary" | "script"
    units: DatabaseSqlUnit[]
    status: DbQueryRunGroupStatus
    run: DbQueryRun | null
    activeStatementExecutionId: DbStatementExecutionId | null
    /** Full ResultSession owner key -> independent current page state. */
    resultPages: Record<string, DbStatementResultPageState>
    startedAt: number
    cancelOutcome: DbQueryCancelResult["outcome"] | null
}

export interface DbQueryState {
    sql: string
    running: boolean
    result: DbQueryResult | null
    error: DbQueryErrorState | null
    elapsedMs: number | null
    /** Exact SQL behind the compatibility result projection. It is never rewritten
     * or re-executed for sorting. Null unless the selected tab produced rows. */
    lastSql: string | null
    sortBy: DbSort | null
    sortBaseRows: DbValue[][] | null
    parseError: DatabaseSqlTargetError | null
    runGroup: DbQueryRunGroup | null
}

/** One recorded SQL run. Canonical runtime ownership is the opaque descriptor;
 * `ranAt` is `Date.now()` (ms), and `error` is set only when `ok` is false. */
export interface DbHistoryEntry {
    sql: string
    ranAt: number
    ok: boolean
    error?: string
    elapsedMs: number
}

// Retired raw-SQL key, retained only so startup can delete it after profile
// hydration. Its value is never read back into renderer state.
export const DB_HISTORY_STORAGE_KEY = "yuzora.db.history.v1"
// Persisted saved connection descriptors (no secrets).
export const DB_CONNECTIONS_STORAGE_KEY = "yuzora.db.connections.v1"
/** Newest-kept entries per key. */
export const DB_HISTORY_LIMIT = 50
/** Defensive per-entry cap so a pathological paste cannot bloat session memory. */
export const DB_HISTORY_SQL_MAX = 5000

interface DbState {
    connections: DbConnection[]
    /** Selection authority. activeConnId below is only its exact live projection. */
    activeDescriptorId: string | null
    activeConnId: string | null
    /** Live-only, most-recently-selected descriptor order. */
    liveMru: string[]
    latestUserIntentToken: number
    latestUserIntentDescriptorId: string | null
    /** The latest intent token associated with an already pending open. */
    openingIntentTokens: Record<string, number>
    /** Persisted descriptors, connected or not. */
    saved: SavedDbConnection[]
    recovery: DbProfileRecoveryRow[]
    profilesLoaded: boolean
    profileError: DbProfileUiErrorCode | null
    /** Session state keyed by descriptor id (mirrors sshStore). */
    sessions: Record<string, DbSessionState>
    /** Canonical runtime metadata buckets, keyed only by descriptorId. */
    tableBuckets: Record<string, DbTable[]>
    tableErrors: Record<string, DbOperationalErrorState | null>
    /** Descriptor → stable qualified object ref → columns/error. */
    columnBuckets: Record<string, Record<string, DbColumn[]>>
    columnErrors: Record<string, Record<string, DbOperationalErrorState | null>>
    /** Legacy connId projection kept for compatibility; never authoritative. */
    tables: Record<string, DbTable[]>
    /** SQL buffers/results are canonically descriptor-owned. Every completion
     * additionally revalidates the exact live connection generation and run id. */
    queryBuckets: Record<string, DbQueryState>
    /** Legacy connId projection kept for compatibility; never authoritative. */
    queries: Record<string, DbQueryState>
    /** Canonical runtime history buckets, keyed only by descriptorId. */
    historyBuckets: Record<string, DbHistoryEntry[]>
    operations: Record<string, DbOperationCounters>
    /** Ephemeral request consumed by DatabaseNavContent. It carries only a
     *  descriptor id; passwords and React callbacks never enter the store. */
    reconnectRequest: DbReconnectRequest | null
    /** Monotonic source for reconnectRequest.token; kept after consumption so
     *  equal descriptor ids always produce a distinct event. */
    reconnectRequestToken: number
    initializeProfiles: () => Promise<void>
    consumeReconnectRequest: (token: number) => void
    /** Open/focus a saved descriptor, or request its password dialog. Row clicks
     *  and context-menu commands both use this exact command path. */
    openOrReconnectSavedConnection: (descriptorId: string) => Promise<DbOpenSavedResult>
    /** Open (or focus) a database from a tagged descriptor; persists the
     *  non-secret descriptor for reconnect. */
    openConfig: (config: DbOpenConfig) => Promise<DbSaveAndConnectOutcome>
    /** SQLite convenience wrapper over openConfig. */
    openConnection: (path: string) => Promise<DbSaveAndConnectOutcome>
    closeConnection: (connId: string) => Promise<void>
    /** Disconnect a descriptor's live connection best-effort, keeping the saved
     *  descriptor and marking the session `disconnected` (mirrors sshStore). */
    disconnect: (id: string) => Promise<boolean>
    /** Edit a saved descriptor in place (same id, recomputed fields). Live
     *  sessions are untouched — new settings apply on the next connect. */
    updateSaved: (id: string, config: DbOpenConfig) => Promise<void>
    /** Forget a saved descriptor (and close its live connection, if any). */
    removeSaved: (id: string) => Promise<void>
    removeCredential: (id: string) => Promise<void>
    recoverProfile: (request: DbProfileRecoveryRequest) => Promise<void>
    setActiveDescriptor: (descriptorId: string) => void
    setActiveConnection: (connId: string) => void
    loadTables: (descriptorId: string) => Promise<void>
    loadColumns: (descriptorId: string, table: DbTable) => Promise<void>
    setSql: (sql: string) => void
    runQuery: (target?: DatabaseSqlTargetRequest) => Promise<void>
    cancelQuery: () => Promise<void>
    selectStatementTab: (statementExecutionId: DbStatementExecutionId) => void
    previousResultPage: (owner: DbResultSessionOwner) => Promise<void>
    nextResultPage: (owner: DbResultSessionOwner) => Promise<void>
    releaseResultSession: (owner: DbResultSessionOwner) => Promise<void>
    /** Sort only the currently loaded page, cycling asc → desc → cleared. */
    sortResult: (columnIndex: number, owner?: DbResultSessionOwner) => Promise<void>
    openTableQuery: (table: DbTable) => Promise<void>
    /** Append a run to a descriptor's session-only history (truncates sql,
     * skips a consecutive duplicate, and caps the list). */
    recordHistory: (key: string, entry: DbHistoryEntry) => void
    reset: () => void
}

function basename(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

function targetKeyOfProfile(target: DbProfileTarget): string {
    return target.kind === "sqlite"
        ? target.path
        : `${target.kind}:${target.host}:${target.port}:${target.database}`
}

/** Same content-derived key, computed from a saved descriptor's own fields (so
 *  legacy records without a stored `targetKey` still resolve — their old id was
 *  this exact string). */
function savedTargetKey(s: SavedDbConnection): string {
    return s.kind === "sqlite"
        ? (s.path ?? s.id)
        : `${s.kind}:${s.host}:${s.port}:${s.database}`
}

function profileTargetOf(config: DbOpenConfig): DbProfileTarget {
    if (config.kind === "sqlite") return { kind: "sqlite", path: config.path }
    if (config.kind === "postgres") {
        return {
            kind: "postgres",
            host: config.host,
            port: config.port,
            database: config.database,
            user: config.user,
            ssl: config.ssl,
            trustCert: config.trustCert
        }
    }
    return {
        kind: "mssql",
        host: config.host,
        port: config.port,
        database: config.database,
        user: config.user,
        trustCert: config.trustCert
    }
}

function profileName(target: DbProfileTarget): string {
    return target.kind === "sqlite" ? basename(target.path) : `${target.database}@${target.host}`
}

function savedFromProfile(profile: DbProfileDescriptor): SavedDbConnection {
    const target = profile.target
    const base: SavedDbConnection = {
        id: profile.descriptorId,
        configGeneration: profile.configGeneration,
        targetKey: targetKeyOfProfile(target),
        kind: target.kind,
        name: profile.name,
        credentialState: profile.credentialState
    }
    if (target.kind === "sqlite") return { ...base, path: target.path }
    if (target.kind === "postgres") {
        return {
            ...base,
            host: target.host,
            port: target.port,
            database: target.database,
            user: target.user,
            ssl: target.ssl,
            trustCert: target.trustCert
        }
    }
    return {
        ...base,
        host: target.host,
        port: target.port,
        database: target.database,
        user: target.user,
        trustCert: target.trustCert
    }
}

function profileFromSaved(saved: SavedDbConnection): DbProfileDescriptor | null {
    if (saved.kind === "sqlite" && saved.path) {
        return {
            descriptorId: saved.id as DbDescriptorId,
            configGeneration: saved.configGeneration ?? 1,
            name: saved.name,
            target: { kind: "sqlite", path: saved.path },
            credentialState: "notRequired"
        }
    }
    if (
        saved.kind !== "sqlite" &&
        saved.host &&
        saved.port &&
        saved.database &&
        saved.user
    ) {
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
        return {
            descriptorId: saved.id as DbDescriptorId,
            configGeneration: saved.configGeneration ?? 1,
            name: saved.name,
            target,
            credentialState: "required"
        }
    }
    return null
}

export function savedConnectionAddress(s: SavedDbConnection): string {
    return s.kind === "sqlite"
        ? (s.path ?? s.name)
        : `${s.user ?? ""}@${s.host ?? ""}:${s.port ?? ""}/${s.database ?? ""}`
}

function connectionFromSaved(connId: string, s: SavedDbConnection): DbConnection {
    return {
        connId,
        kind: s.kind,
        name: s.name,
        descriptorId: s.id,
        targetKey: s.targetKey,
        title: savedConnectionAddress(s)
    }
}

function connectionFromLive(live: DbLiveConnection, saved: SavedDbConnection): DbConnection {
    return {
        ...connectionFromSaved(live.connectionId, saved),
        connectionGeneration: live.connectionGeneration
    }
}

// Re-project through the field whitelist so nothing outside the descriptor (in
// particular any secret) can survive a load/save round-trip.
function sanitizeSaved(s: SavedDbConnection): SavedDbConnection {
    // targetKey is always recomputed from the descriptor's own fields, so legacy
    // records (persisted before targetKey existed, where id === targetKey) can
    // still map the retired target-key history during one-time migration.
    const out: SavedDbConnection = {
        id: s.id,
        configGeneration: s.configGeneration ?? 1,
        targetKey: savedTargetKey(s),
        kind: s.kind,
        name: s.name,
        credentialState: s.credentialState
    }
    if (s.kind === "sqlite") {
        if (s.path) out.path = s.path
        return out
    }
    out.host = s.host
    out.port = s.port
    out.database = s.database
    out.user = s.user
    if (s.kind === "postgres") {
        out.ssl = s.ssl
        out.trustCert = s.trustCert
    }
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

const DB_PROFILE_ERROR_CODES: ReadonlySet<string> = new Set<DbProfileErrorCode>([
    "repositoryUnavailable",
    "vaultMissing",
    "vaultDenied",
    "vaultUnavailable",
    "vaultCorrupt",
    "vaultWriteFailed",
    "vaultDeleteFailed",
    "profileNotFound",
    "pendingOperationConflict",
    "recoveryNotFound",
    "recoveryActionInvalid",
    "credentialRequired",
    "lifecycleCancelFailed",
    "lifecycleCloseFailed",
    "connectionFailed",
    "connectionBusy",
    "serverDisconnected",
    "metadataFailed",
    "queryFailed",
    "staleConnection",
    "sqlitePathMissing",
    "sqlitePathNotFile",
    "sqlitePathUnreadable",
    "sqlitePathInvalid",
    "sqliteOpenFailed",
    "invalidRequest"
])

const DB_OPERATIONAL_ERROR_CODES: ReadonlySet<string> = new Set<DbOperationalErrorCode>([
    "connectionFailed",
    "connectionBusy",
    "serverDisconnected",
    "metadataFailed",
    "queryFailed",
    "staleConnection",
    "sqlitePathMissing",
    "sqlitePathNotFile",
    "sqlitePathUnreadable",
    "sqlitePathInvalid",
    "sqliteOpenFailed"
])

const DB_ERROR_ENGINES = new Set(["sqlite", "postgres", "mssql", "yuzora"])
const DB_ERROR_RETRYABILITY = new Set(["retryable", "notRetryable", "unknown"])

/** Convert the typed IPC error envelope to a display-safe token. Unknown or
 * malformed rejections deliberately collapse to a fixed fallback. */
function postgresProfileUiErrorCode(error: unknown): DbProfileUiErrorCode | null {
    if (typeof error !== "object" || error === null) return null
    const envelope = error as Record<string, unknown>
    if (envelope.code !== "connectionFailed" || typeof envelope.message !== "string") return null
    const diagnostic = databaseErrorFromOperationalEnvelope(error)
    if (diagnostic?.engine !== "postgres" || diagnostic.code === null) return null
    if (diagnostic.code === "dnsFailed") return "connectionDnsFailed"
    if (diagnostic.code === "tlsFailed") return "connectionTlsFailed"
    if (diagnostic.code === "connectionTimedOut") return "connectionTimedOut"
    if (!/^[0-9A-Z]{5}$/.test(diagnostic.code)) return null
    return diagnostic.code.startsWith("28")
        ? "connectionAuthenticationFailed"
        : "connectionServerRejected"
}

export function dbProfileUiErrorCode(error: unknown): DbProfileUiErrorCode {
    if (typeof error !== "object" || error === null) return "unknown"
    const postgresCode = postgresProfileUiErrorCode(error)
    if (postgresCode) return postgresCode
    const code = (error as { code?: unknown }).code
    return typeof code === "string" && DB_PROFILE_ERROR_CODES.has(code)
        ? (code as DbProfileErrorCode)
        : "unknown"
}

export function dbProfileNeedsCredentialPrompt(code: DbProfileUiErrorCode): boolean {
    return code === "credentialRequired" ||
        code === "connectionAuthenticationFailed" ||
        code === "vaultMissing" ||
        code === "vaultCorrupt" ||
        code === "vaultDenied"
}

function operationalErrorCode(
    error: unknown,
    fallback: "metadataFailed" | "queryFailed" | "connectionFailed"
): DbOperationalErrorCode {
    if (typeof error !== "object" || error === null) return fallback
    const code = (error as { code?: unknown }).code
    return typeof code === "string" && DB_OPERATIONAL_ERROR_CODES.has(code)
        ? (code as DbOperationalErrorCode)
        : fallback
}

function databaseErrorFromOperationalEnvelope(error: unknown): DbError | null {
    if (typeof error !== "object" || error === null) return null
    const nested = (error as { error?: unknown }).error
    if (typeof nested !== "object" || nested === null) return null
    const candidate = nested as Record<string, unknown>
    if (
        typeof candidate.engine !== "string" ||
        !DB_ERROR_ENGINES.has(candidate.engine) ||
        typeof candidate.message !== "string" ||
        !(candidate.code === null || typeof candidate.code === "string") ||
        !(candidate.detail === null || typeof candidate.detail === "string") ||
        !(candidate.hint === null || typeof candidate.hint === "string") ||
        typeof candidate.retryability !== "string" ||
        !DB_ERROR_RETRYABILITY.has(candidate.retryability)
    ) return null

    if (candidate.position !== null) {
        if (typeof candidate.position !== "object" || candidate.position === null) return null
        const position = candidate.position as Record<string, unknown>
        for (const field of ["offset", "line", "column"] as const) {
            if (!(position[field] === null || typeof position[field] === "number")) return null
        }
    }
    return nested as DbError
}

function operationalErrorState(
    error: unknown,
    fallback: "metadataFailed" | "queryFailed" | "connectionFailed"
): DbOperationalErrorState {
    return {
        code: operationalErrorCode(error, fallback),
        databaseError: databaseErrorFromOperationalEnvelope(error)
    }
}

const EMPTY_OPERATION_COUNTERS: DbOperationCounters = {
    open: 0,
    edit: 0,
    remove: 0,
    disconnect: 0,
    tables: 0,
    columns: 0,
    query: 0,
    page: 0
}

let columnRequestSequence = 0
const currentColumnRequests = new Map<string, number>()
let resultPageRequestSequence = 0
const currentResultPageRequests = new Map<string, number>()

function columnRequestKey(descriptorId: string, objectKey: string): string {
    return JSON.stringify([descriptorId, objectKey])
}

function beginColumnRequest(descriptorId: string, objectKey: string): number {
    const token = ++columnRequestSequence
    currentColumnRequests.set(columnRequestKey(descriptorId, objectKey), token)
    return token
}

function finishColumnRequest(descriptorId: string, objectKey: string, token: number): void {
    const key = columnRequestKey(descriptorId, objectKey)
    if (currentColumnRequests.get(key) === token) currentColumnRequests.delete(key)
}

export function resultPageKey(owner: DbResultSessionOwner): string {
    return JSON.stringify([
        owner.descriptorId,
        owner.connectionId,
        owner.connectionGeneration,
        owner.queryRunId,
        owner.statementExecutionId,
        owner.resultSessionId
    ])
}

function beginResultPageRequest(owner: DbResultSessionOwner): number {
    const token = ++resultPageRequestSequence
    currentResultPageRequests.set(resultPageKey(owner), token)
    return token
}

function finishResultPageRequest(owner: DbResultSessionOwner, token: number): void {
    const key = resultPageKey(owner)
    if (currentResultPageRequests.get(key) === token) currentResultPageRequests.delete(key)
}

function operationCountersFor(
    operations: Record<string, DbOperationCounters>,
    descriptorId: string
): DbOperationCounters {
    return operations[descriptorId] ?? EMPTY_OPERATION_COUNTERS
}

function bumpOperations(
    operations: Record<string, DbOperationCounters>,
    descriptorId: string,
    primary: DbOperationSlot,
    invalidates: DbOperationSlot[] = []
): { operations: Record<string, DbOperationCounters>; token: number } {
    const previous = operationCountersFor(operations, descriptorId)
    const next = { ...previous }
    for (const slot of new Set([primary, ...invalidates])) next[slot] += 1
    return {
        operations: { ...operations, [descriptorId]: next },
        token: next[primary]
    }
}

function savedConfigGeneration(saved: SavedDbConnection | undefined): number | null {
    return saved ? (saved.configGeneration ?? 1) : null
}

function identityOf(connection: DbConnection | undefined): DbConnectionIdentity | null {
    if (!connection?.connectionGeneration) return null
    return {
        descriptorId: connection.descriptorId as DbDescriptorId,
        connectionId: connection.connId as DbConnectionId,
        connectionGeneration: connection.connectionGeneration
    }
}

function exactConnection(
    state: Pick<DbState, "connections">,
    identity: DbConnectionIdentity
): DbConnection | null {
    return state.connections.find((connection) =>
        connection.descriptorId === identity.descriptorId &&
        connection.connId === identity.connectionId &&
        connection.connectionGeneration === identity.connectionGeneration
    ) ?? null
}

function liveConnectionForDescriptor(
    state: Pick<DbState, "connections">,
    descriptorId: string
): DbConnection | null {
    return state.connections.find((connection) => connection.descriptorId === descriptorId) ?? null
}

function operationStillCurrent(
    state: Pick<DbState, "operations" | "saved" | "connections">,
    descriptorId: string,
    configGeneration: number,
    slot: DbOperationSlot,
    token: number,
    identity?: DbConnectionIdentity
): boolean {
    return operationCountersFor(state.operations, descriptorId)[slot] === token &&
        savedConfigGeneration(state.saved.find((profile) => profile.id === descriptorId)) === configGeneration &&
        (!identity || exactConnection(state, identity) !== null)
}

function columnRequestStillCurrent(
    state: Pick<DbState, "operations" | "saved" | "connections">,
    descriptorId: string,
    objectKey: string,
    configGeneration: number,
    columnsEpoch: number,
    requestToken: number,
    identity: DbConnectionIdentity
): boolean {
    return currentColumnRequests.get(columnRequestKey(descriptorId, objectKey)) === requestToken &&
        operationStillCurrent(
            state,
            descriptorId,
            configGeneration,
            "columns",
            columnsEpoch,
            identity
        )
}

function projectedConnId(
    connections: DbConnection[],
    activeDescriptorId: string | null
): string | null {
    if (!activeDescriptorId) return null
    return connections.find((connection) => connection.descriptorId === activeDescriptorId)?.connId ?? null
}

function liveOnlyMru(connections: DbConnection[], mru: string[]): string[] {
    const live = new Set(connections.map((connection) => connection.descriptorId))
    const seen = new Set<string>()
    const ordered = mru.filter((descriptorId) => {
        if (!live.has(descriptorId) || seen.has(descriptorId)) return false
        seen.add(descriptorId)
        return true
    })
    for (const connection of connections) {
        if (!seen.has(connection.descriptorId)) {
            seen.add(connection.descriptorId)
            ordered.push(connection.descriptorId)
        }
    }
    return ordered
}

function touchLiveMru(connections: DbConnection[], mru: string[], descriptorId: string): string[] {
    return liveOnlyMru(connections, [descriptorId, ...mru.filter((id) => id !== descriptorId)])
}

function appendLiveMru(connections: DbConnection[], mru: string[], descriptorId: string): string[] {
    return liveOnlyMru(connections, [...mru.filter((id) => id !== descriptorId), descriptorId])
}

function fallbackActiveDescriptor(connections: DbConnection[], mru: string[]): string | null {
    const live = new Set(connections.map((connection) => connection.descriptorId))
    return mru.find((descriptorId) => live.has(descriptorId)) ?? connections[0]?.descriptorId ?? null
}

function reconciledSnapshotFields(
    current: DbState,
    snapshot: DbProfileLoadResult
) {
    const profiles = snapshot.profiles.map(savedFromProfile)
    const profileById = new Map(profiles.map((profile) => [profile.id, profile]))
    const blocked = new Set(snapshot.recovery.map((row) => row.descriptorId))
    const connections = current.connections.filter((connection) => {
        const profile = profileById.get(connection.descriptorId)
        return Boolean(
            profile &&
                !blocked.has(connection.descriptorId as DbDescriptorId) &&
                profile.credentialState !== "required" &&
                profile.credentialState !== "unavailable"
        )
    })
    const liveIds = new Set(connections.map((connection) => connection.connId))
    const profileIds = new Set(profiles.map((profile) => profile.id))
    const tables = Object.fromEntries(
        Object.entries(current.tables).filter(([connId]) => liveIds.has(connId))
    )
    const queries = Object.fromEntries(
        Object.entries(current.queries).filter(([connId]) => liveIds.has(connId))
    )
    const tableBuckets = Object.fromEntries(
        Object.entries(current.tableBuckets).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    const tableErrors = Object.fromEntries(
        Object.entries(current.tableErrors).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    const columnBuckets = Object.fromEntries(
        Object.entries(current.columnBuckets).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    const columnErrors = Object.fromEntries(
        Object.entries(current.columnErrors).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    const queryBuckets = Object.fromEntries(
        Object.entries(current.queryBuckets).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    // History is session-only. Hydration may retain entries created during this
    // app session, but retired browser history is never copied back into state.
    const historyBuckets = Object.fromEntries(profiles.map((profile) => [
        profile.id,
        (current.historyBuckets[profile.id] ?? []).map((entry) => ({ ...entry }))
    ]))
    const operations = Object.fromEntries(
        Object.entries(current.operations).filter(([descriptorId]) => profileIds.has(descriptorId))
    )
    const sessions: Record<string, DbSessionState> = {}
    for (const profile of profiles) {
        const live = connections.find((connection) => connection.descriptorId === profile.id)
        const previous = current.sessions[profile.id]
        sessions[profile.id] = live
            ? {
                  descriptorId: profile.id,
                  connId: live.connId,
                  status: previous?.status === "connecting" ? "connecting" : "connected",
                  error: null
              }
            : {
                  descriptorId: profile.id,
                  connId: null,
                  status: previous?.status === "error" ? "error" : "disconnected",
                  error: previous?.status === "error" ? previous.error : null
              }
    }
    const liveMru = liveOnlyMru(connections, current.liveMru)
    const activeDescriptorId = current.activeDescriptorId &&
        connections.some((connection) => connection.descriptorId === current.activeDescriptorId)
        ? current.activeDescriptorId
        : fallbackActiveDescriptor(connections, liveMru)
    return {
        saved: profiles,
        recovery: snapshot.recovery,
        profilesLoaded: true,
        connections,
        tables,
        queries,
        tableBuckets,
        tableErrors,
        columnBuckets,
        columnErrors,
        queryBuckets,
        historyBuckets,
        operations,
        sessions,
        liveMru,
        activeDescriptorId,
        activeConnId: projectedConnId(connections, activeDescriptorId)
    }
}

async function bestEffortProfileSnapshot(): Promise<DbProfileLoadResult | null> {
    try {
        return await dbProfileList()
    } catch {
        return null
    }
}

/** Stable empty-state reference: the default for a descriptor with no query
 *  bucket yet, so selectors reading it don't allocate a new object per call. */
const EMPTY_DB_QUERY: DbQueryState = {
    sql: "",
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

/** Resolve the descriptor-owned bucket, with a legacy connId projection fallback. */
export function queryFor(
    s: {
        queryBuckets?: Record<string, DbQueryState>
        queries: Record<string, DbQueryState>
    },
    descriptorId: string | null
): DbQueryState {
    return (descriptorId && (s.queryBuckets?.[descriptorId] ?? s.queries[descriptorId])) || EMPTY_DB_QUERY
}

function queryRunMatchesOwner(run: DbQueryRun, owner: DbQueryRunOwner): boolean {
    return queryRunOwnerMatches(run, owner)
}

function queryRunOwnerMatches(
    candidate: DbQueryRunOwner,
    expected: DbQueryRunOwner
): boolean {
    return candidate.descriptorId === expected.descriptorId &&
        candidate.connectionId === expected.connectionId &&
        candidate.connectionGeneration === expected.connectionGeneration &&
        candidate.queryRunId === expected.queryRunId
}

function resultSessionForStatement(statement: DbStatementExecution): DbResultSession | null {
    if (statement.result.kind === "rows") return statement.result.resultSession
    if (statement.result.kind === "resultLimitReached") return statement.result.resultSession
    return null
}

function resultOwnerMatches(
    candidate: DbResultSessionOwner,
    expected: DbResultSessionOwner
): boolean {
    return candidate.descriptorId === expected.descriptorId &&
        candidate.connectionId === expected.connectionId &&
        candidate.connectionGeneration === expected.connectionGeneration &&
        candidate.queryRunId === expected.queryRunId &&
        candidate.statementExecutionId === expected.statementExecutionId &&
        candidate.resultSessionId === expected.resultSessionId
}

function resultPageStatesForRun(run: DbQueryRun): Record<string, DbStatementResultPageState> {
    const resultPages: Record<string, DbStatementResultPageState> = {}
    for (const statement of run.statements) {
        const session = resultSessionForStatement(statement)
        if (!session) continue
        const page = session.initialPage
        resultPages[resultPageKey(session.owner)] = {
            page,
            loading: false,
            pageError: null,
            released: page.lifecycle === "released",
            sort: null,
            sortBaseRows: page.rows
        }
    }
    return resultPages
}

export function resultPageStateForStatement(
    group: DbQueryRunGroup | null,
    statement: DbStatementExecution
): DbStatementResultPageState | null {
    const session = resultSessionForStatement(statement)
    return session && group ? (group.resultPages[resultPageKey(session.owner)] ?? null) : null
}

function activeStreamingResultOwner(group: DbQueryRunGroup): DbResultSessionOwner | null {
    if (group.status !== "settled" || !group.run || !group.activeStatementExecutionId) return null
    const statement = group.run.statements.find((candidate) =>
        candidate.statementExecutionId === group.activeStatementExecutionId
    )
    if (!statement) return null
    const session = resultSessionForStatement(statement)
    const pageState = resultPageStateForStatement(group, statement)
    return session && pageState && !pageState.released && pageState.page.lifecycle === "streaming"
        ? session.owner
        : null
}

export function queryRunGroupIsCancellable(group: DbQueryRunGroup | null): boolean {
    return group?.status === "running" || Boolean(group && activeStreamingResultOwner(group))
}

function exactResultPageState(
    query: DbQueryState,
    owner: DbResultSessionOwner
): { statement: DbStatementExecution; state: DbStatementResultPageState } | null {
    const group = query.runGroup
    if (!group?.run || !queryRunMatchesOwner(group.run, owner)) return null
    const statement = group.run.statements.find((candidate) =>
        candidate.statementExecutionId === owner.statementExecutionId
    )
    const session = statement ? resultSessionForStatement(statement) : null
    if (!statement || !session || !resultOwnerMatches(session.owner, owner)) return null
    const state = group.resultPages[resultPageKey(owner)]
    return state && resultOwnerMatches(state.page.owner, owner) ? { statement, state } : null
}

function preferredStatement(run: DbQueryRun): DbStatementExecution {
    return run.statements.find((statement) =>
        statement.result.kind === "error" ||
        statement.result.kind === "cancelled" ||
        statement.result.kind === "resultLimitReached"
    ) ?? run.statements.find((statement) => statement.result.kind === "rows") ?? run.statements[0]
}

function legacyProjectionForStatement(
    statement: DbStatementExecution,
    pageState: DbStatementResultPageState | null = null
): {
    result: DbQueryResult | null
    error: DbQueryErrorState | null
    lastSql: string | null
} {
    if (statement.result.kind === "rows" || statement.result.kind === "resultLimitReached") {
        const session = statement.result.resultSession
        if (!session) return { result: null, error: null, lastSql: null }
        const page = pageState?.page ?? session.initialPage
        return {
            result: {
                kind: "select",
                columns: page.columns,
                rows: page.rows,
                truncated: page.resultLimitReached || page.hasNext,
                affectedRows: statement.result.affectedRows,
                effectOutcome: page.effectOutcome
            },
            error: null,
            lastSql: statement.sql
        }
    }
    if (statement.result.kind === "execute") {
        return {
            result: {
                kind: "execute",
                affectedRows: statement.result.affectedRows,
                effectOutcome: statement.effectOutcome
            },
            error: null,
            lastSql: null
        }
    }
    if (statement.result.kind === "error") {
        return {
            result: null,
            error: {
                code: "queryFailed",
                databaseError: statement.result.error,
                executedSql: statement.sql
            },
            lastSql: null
        }
    }
    return { result: null, error: null, lastSql: null }
}

function resultSessionOwners(run: DbQueryRun): Array<Parameters<typeof dbResultSessionRelease>[0]> {
    return run.statements.flatMap((statement) => {
        if (statement.result.kind === "rows" && statement.result.resultSession) {
            return [statement.result.resultSession.owner]
        }
        if (statement.result.kind === "resultLimitReached") {
            return [statement.result.resultSession.owner]
        }
        return []
    })
}

function compareNumericText(left: string, right: string): number | null {
    const pattern = /^([+-]?)(\d+)(?:\.(\d+))?$/
    const leftMatch = pattern.exec(left)
    const rightMatch = pattern.exec(right)
    if (!leftMatch || !rightMatch) return null
    const normalize = (match: RegExpExecArray) => ({
        negative: match[1] === "-",
        integer: match[2].replace(/^0+(?=\d)/, ""),
        fraction: (match[3] ?? "").replace(/0+$/, "")
    })
    const a = normalize(leftMatch)
    const b = normalize(rightMatch)
    if (a.negative !== b.negative) return a.negative ? -1 : 1
    const direction = a.negative ? -1 : 1
    if (a.integer.length !== b.integer.length) {
        return (a.integer.length < b.integer.length ? -1 : 1) * direction
    }
    const integerOrder = a.integer.localeCompare(b.integer)
    if (integerOrder !== 0) return integerOrder * direction
    const width = Math.max(a.fraction.length, b.fraction.length)
    return a.fraction.padEnd(width, "0").localeCompare(b.fraction.padEnd(width, "0")) * direction
}

function compareDbValues(left: DbValue | undefined, right: DbValue | undefined): number {
    if (!left && !right) return 0
    if (!left) return 1
    if (!right) return -1
    if (left.kind === "null" || right.kind === "null") {
        if (left.kind === right.kind) return 0
        return left.kind === "null" ? 1 : -1
    }
    if (
        (left.kind === "integer" || left.kind === "decimal") &&
        (right.kind === "integer" || right.kind === "decimal")
    ) {
        const numeric = compareNumericText(left.value, right.value)
        if (numeric !== null) return numeric
    }
    const leftText = left.kind === "binary" ? left.hex : String(left.value)
    const rightText = right.kind === "binary" ? right.hex : String(right.value)
    return leftText.localeCompare(rightText)
}

function sortedPageRows(rows: DbValue[][], sort: DbSort | null): DbValue[][] {
    if (!sort) return rows
    return [...rows].sort((left, right) => {
        const order = compareDbValues(left[sort.columnIndex], right[sort.columnIndex])
        return sort.dir === "asc" ? order : -order
    })
}

function queryWithResultPageState(
    query: DbQueryState,
    owner: DbResultSessionOwner,
    nextPageState: DbStatementResultPageState
): DbQueryState | null {
    const exact = exactResultPageState(query, owner)
    const group = query.runGroup
    if (!exact || !group) return null
    const nextGroup: DbQueryRunGroup = {
        ...group,
        resultPages: {
            ...group.resultPages,
            [resultPageKey(owner)]: nextPageState
        }
    }
    if (group.activeStatementExecutionId !== owner.statementExecutionId) {
        return { ...query, runGroup: nextGroup }
    }
    const projection = legacyProjectionForStatement(exact.statement, nextPageState)
    return {
        ...query,
        result: projection.result,
        error: projection.error,
        lastSql: projection.lastSql,
        sortBy: nextPageState.sort,
        sortBaseRows: nextPageState.sort ? nextPageState.sortBaseRows : null,
        runGroup: nextGroup
    }
}

function resultPageRequestStillCurrent(
    state: DbState,
    owner: DbResultSessionOwner,
    configGeneration: number,
    pageEpoch: number,
    requestToken: number
): boolean {
    return currentResultPageRequests.get(resultPageKey(owner)) === requestToken &&
        operationStillCurrent(
            state,
            owner.descriptorId,
            configGeneration,
            "page",
            pageEpoch,
            owner
        ) &&
        exactResultPageState(queryFor(state, owner.descriptorId), owner) !== null
}

/** Detect the retired key without reading its raw SQL/error value. A storage
 * access failure keeps cleanup pending so a later explicit initialization can
 * retry after browser storage access is restored. */
function hasLegacyHistoryStorageKey(): boolean {
    try {
        for (let index = 0; index < localStorage.length; index += 1) {
            if (localStorage.key(index) === DB_HISTORY_STORAGE_KEY) return true
        }
        return false
    } catch {
        return true
    }
}

let pendingLegacyHistoryCleanup = hasLegacyHistoryStorageKey()

function completeLegacyHistoryMigration(): DbProfileUiErrorCode | null {
    if (!pendingLegacyHistoryCleanup) return null
    try {
        localStorage.removeItem(DB_HISTORY_STORAGE_KEY)
        pendingLegacyHistoryCleanup = false
        return null
    } catch {
        return "legacyHistoryCleanupFailed"
    }
}

export const dbInitialState = {
    connections: [] as DbConnection[],
    activeDescriptorId: null as string | null,
    activeConnId: null as string | null,
    liveMru: [] as string[],
    latestUserIntentToken: 0,
    latestUserIntentDescriptorId: null as string | null,
    openingIntentTokens: {} as Record<string, number>,
    sessions: {} as Record<string, DbSessionState>,
    tableBuckets: {} as Record<string, DbTable[]>,
    tableErrors: {} as Record<string, DbOperationalErrorState | null>,
    columnBuckets: {} as Record<string, Record<string, DbColumn[]>>,
    columnErrors: {} as Record<string, Record<string, DbOperationalErrorState | null>>,
    tables: {} as Record<string, DbTable[]>,
    queryBuckets: {} as Record<string, DbQueryState>,
    queries: {} as Record<string, DbQueryState>,
    historyBuckets: {} as Record<string, DbHistoryEntry[]>,
    operations: {} as Record<string, DbOperationCounters>,
    reconnectRequest: null as DbReconnectRequest | null,
    reconnectRequestToken: 0,
    recovery: [] as DbProfileRecoveryRow[],
    profilesLoaded: false,
    profileError: null as DbProfileUiErrorCode | null
}

// React StrictMode may mount the database navigation twice before the first
// startup read settles. Coalesce those callers so an older pre-import snapshot
// can never race a confirmed legacy import and overwrite it. The shared promise
// is cleared on every settlement, allowing an explicit later retry.
let profileInitializationInFlight: Promise<void> | null = null

export const useDbStore = create<DbState>()((set, get) => {
    function beginUserIntent(descriptorId: string | null): number {
        const token = get().latestUserIntentToken + 1
        set({
            latestUserIntentToken: token,
            latestUserIntentDescriptorId: descriptorId
        })
        return token
    }

    function beginOperation(
        descriptorId: string,
        slot: DbOperationSlot,
        invalidates: DbOperationSlot[] = []
    ): number {
        const bumped = bumpOperations(get().operations, descriptorId, slot, invalidates)
        set({ operations: bumped.operations })
        return bumped.token
    }

    async function disconnectUnpublishedLive(live: DbLiveConnection): Promise<void> {
        try {
            await dbProfileDisconnect(live)
        } catch (error) {
            set({ profileError: dbProfileUiErrorCode(error) })
        }
    }

    function projectExactServerDisconnect(
        identity: DbConnectionIdentity,
        queryError?: DbQueryErrorState,
        sessionError: DbProfileUiErrorCode | null = "serverDisconnected"
    ): void {
        set((current) => {
            if (!exactConnection(current, identity)) return {}
            const descriptorId = identity.descriptorId
            const connections = current.connections.filter((connection) =>
                connection.descriptorId !== descriptorId
            )
            const tables = { ...current.tables }
            const queries = { ...current.queries }
            delete tables[identity.connectionId]
            delete queries[identity.connectionId]
            const liveMru = liveOnlyMru(
                connections,
                current.liveMru.filter((id) => id !== descriptorId)
            )
            const activeDescriptorId = current.activeDescriptorId === descriptorId
                ? fallbackActiveDescriptor(connections, liveMru)
                : current.activeDescriptorId
            const existingQuery = current.queryBuckets[descriptorId]
            return {
                connections,
                tables,
                queries,
                liveMru,
                activeDescriptorId,
                activeConnId: projectedConnId(connections, activeDescriptorId),
                sessions: {
                    ...current.sessions,
                    [descriptorId]: {
                        descriptorId,
                        connId: null,
                        status: "disconnected",
                        error: sessionError
                    }
                },
                queryBuckets: existingQuery
                    ? {
                          ...current.queryBuckets,
                          [descriptorId]: {
                              ...existingQuery,
                              running: false,
                              error: queryError ?? existingQuery.error
                          }
                      }
                    : current.queryBuckets
            }
        })
    }

    async function requestResultPage(
        owner: DbResultSessionOwner,
        operation: "previous" | "next" | "release"
    ): Promise<void> {
        const initial = get()
        const configGeneration = savedConfigGeneration(
            initial.saved.find((profile) => profile.id === owner.descriptorId)
        )
        const pageEpoch = operationCountersFor(initial.operations, owner.descriptorId).page
        const exact = exactResultPageState(queryFor(initial, owner.descriptorId), owner)
        if (
            configGeneration === null ||
            !exact ||
            exact.state.loading ||
            !operationStillCurrent(
                initial,
                owner.descriptorId,
                configGeneration,
                "page",
                pageEpoch,
                owner
            )
        ) return
        if (operation === "previous" && !exact.state.page.hasPrevious) return
        if (
            operation === "next" &&
            (
                exact.state.released ||
                !exact.state.page.hasNext ||
                exact.state.page.lifecycle === "released" ||
                exact.state.page.lifecycle === "cancelled" ||
                exact.state.page.lifecycle === "error"
            )
        ) return
        if (operation === "release" && exact.state.released) return

        const requestToken = beginResultPageRequest(owner)
        let started = false
        set((snapshot) => {
            if (!resultPageRequestStillCurrent(
                snapshot,
                owner,
                configGeneration,
                pageEpoch,
                requestToken
            )) return {}
            const current = queryFor(snapshot, owner.descriptorId)
            const currentPage = exactResultPageState(current, owner)
            if (!currentPage || currentPage.state.loading) return {}
            const next = queryWithResultPageState(current, owner, {
                ...currentPage.state,
                loading: true,
                pageError: null
            })
            if (!next) return {}
            started = true
            return {
                queryBuckets: { ...snapshot.queryBuckets, [owner.descriptorId]: next },
                queries: { ...snapshot.queries, [owner.connectionId]: next }
            }
        })
        if (!started) {
            finishResultPageRequest(owner, requestToken)
            return
        }

        try {
            const response = operation === "previous"
                ? await dbResultPagePrevious(owner)
                : operation === "next"
                  ? await dbResultPageNext(owner)
                  : await dbResultSessionRelease(owner)
            if (!resultOwnerMatches(response.owner, owner)) {
                throw { code: "staleConnection", message: "result page owner mismatch" }
            }
            set((snapshot) => {
                if (!resultPageRequestStillCurrent(
                    snapshot,
                    owner,
                    configGeneration,
                    pageEpoch,
                    requestToken
                )) return {}
                const current = queryFor(snapshot, owner.descriptorId)
                const currentPage = exactResultPageState(current, owner)
                if (!currentPage) return {}
                const sortBaseRows = response.rows
                const released = operation === "release" || response.lifecycle === "released"
                const page: DbResultPage = {
                    ...response,
                    rows: sortedPageRows(sortBaseRows, currentPage.state.sort),
                    hasNext: released ? false : response.hasNext,
                    lifecycle: released ? "released" : response.lifecycle
                }
                const next = queryWithResultPageState(current, owner, {
                    ...currentPage.state,
                    page,
                    loading: false,
                    pageError: null,
                    released,
                    sortBaseRows
                })
                if (!next) return {}
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [owner.descriptorId]: next },
                    queries: { ...snapshot.queries, [owner.connectionId]: next }
                }
            })
        } catch (error) {
            set((snapshot) => {
                if (!resultPageRequestStillCurrent(
                    snapshot,
                    owner,
                    configGeneration,
                    pageEpoch,
                    requestToken
                )) return {}
                const current = queryFor(snapshot, owner.descriptorId)
                const currentPage = exactResultPageState(current, owner)
                if (!currentPage) return {}
                const next = queryWithResultPageState(current, owner, {
                    ...currentPage.state,
                    loading: false,
                    pageError: operationalErrorState(error, "queryFailed")
                })
                if (!next) return {}
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [owner.descriptorId]: next },
                    queries: { ...snapshot.queries, [owner.connectionId]: next }
                }
            })
        } finally {
            set((snapshot) => {
                if (currentResultPageRequests.get(resultPageKey(owner)) !== requestToken) return {}
                const current = queryFor(snapshot, owner.descriptorId)
                const currentPage = exactResultPageState(current, owner)
                if (!currentPage?.state.loading) return {}
                const next = queryWithResultPageState(current, owner, {
                    ...currentPage.state,
                    loading: false
                })
                return next
                    ? { queryBuckets: { ...snapshot.queryBuckets, [owner.descriptorId]: next } }
                    : {}
            })
            finishResultPageRequest(owner, requestToken)
        }
    }

    return ({
    ...dbInitialState,
    saved: loadSavedConnections(),

    consumeReconnectRequest: (token) =>
        set((current) => current.reconnectRequest?.token === token
            ? { reconnectRequest: null }
            : {}),

    initializeProfiles: () => {
        if (profileInitializationInFlight) return profileInitializationInFlight

        const run = (async () => {
            try {
                let snapshot = await dbProfileList()
                const legacyProfiles = loadSavedConnections()
                    .map(profileFromSaved)
                    .filter((profile): profile is DbProfileDescriptor => profile !== null)
                if (legacyProfiles.length > 0) {
                    snapshot = await dbProfileImportLegacy({ profiles: legacyProfiles })
                    set((current) => ({
                        ...reconciledSnapshotFields(current, snapshot),
                        profileError: null
                    }))
                    // Rust has confirmed the atomic repository replacement. A failed
                    // import intentionally leaves the v1 key untouched for retry.
                    let cleanupError: DbProfileUiErrorCode | null = null
                    try {
                        localStorage.removeItem(DB_CONNECTIONS_STORAGE_KEY)
                    } catch {
                        cleanupError = "legacyCleanupFailed"
                    }
                    cleanupError = completeLegacyHistoryMigration() ?? cleanupError
                    if (cleanupError) set({ profileError: cleanupError })
                    return
                }
                set((current) => ({
                    ...reconciledSnapshotFields(current, snapshot),
                    profileError: null
                }))
                const cleanupError = completeLegacyHistoryMigration()
                if (cleanupError) set({ profileError: cleanupError })
            } catch (error) {
                set({
                    profilesLoaded: true,
                    profileError: dbProfileUiErrorCode(error)
                })
            }
        })()
        const shared = run.finally(() => {
            if (profileInitializationInFlight === shared) {
                profileInitializationInFlight = null
            }
        })
        profileInitializationInFlight = shared
        return shared
    },

    openOrReconnectSavedConnection: async (descriptorId) => {
        let state = get()
        const descriptor = state.saved.find((entry) => entry.id === descriptorId)
        // A stale request must never fall back to whichever connection is active.
        if (!descriptor) return { outcome: "cancelled" }
        const intentToken = beginUserIntent(descriptorId)
        if (state.sessions[descriptorId]?.status === "connecting") {
            set((current) => ({
                openingIntentTokens: {
                    ...current.openingIntentTokens,
                    [descriptorId]: intentToken
                }
            }))
            return { outcome: "cancelled" }
        }

        state = get()
        const live = liveConnectionForDescriptor(state, descriptorId)
        if (live) {
            if (descriptorId === state.activeDescriptorId) return { outcome: "cancelled" }
            state.setActiveDescriptor(descriptorId)
            return { outcome: "completed" }
        }

        const configGeneration = savedConfigGeneration(descriptor)
        if (configGeneration === null) return { outcome: "cancelled" }
        const openToken = beginOperation(
            descriptorId,
            "open",
            ["disconnect", "tables", "columns", "query", "page"]
        )
        set((current) => ({
            openingIntentTokens: {
                ...current.openingIntentTokens,
                [descriptorId]: intentToken
            },
            sessions: {
                ...current.sessions,
                [descriptorId]: {
                    descriptorId,
                    connId: null,
                    status: "connecting",
                    error: null
                }
            }
        }))
        try {
            const liveConnection = await dbProfileOpen(descriptorId as DbDescriptorId)
            if (!operationStillCurrent(
                get(), descriptorId, configGeneration, "open", openToken
            )) {
                await disconnectUnpublishedLive(liveConnection)
                return { outcome: "cancelled" }
            }
            const connection = connectionFromLive(liveConnection, descriptor)
            set((current) => {
                if (!operationStillCurrent(
                    current, descriptorId, configGeneration, "open", openToken
                )) return {}
                const connections = [
                    ...current.connections.filter((item) => item.descriptorId !== descriptorId),
                    connection
                ]
                const shouldActivate =
                    current.latestUserIntentDescriptorId === descriptorId &&
                    current.openingIntentTokens[descriptorId] === current.latestUserIntentToken
                const liveMru = shouldActivate
                    ? touchLiveMru(connections, current.liveMru, descriptorId)
                    : appendLiveMru(connections, current.liveMru, descriptorId)
                const activeDescriptorId = shouldActivate
                    ? descriptorId
                    : current.activeDescriptorId && connections.some(
                        (item) => item.descriptorId === current.activeDescriptorId
                    )
                        ? current.activeDescriptorId
                        : null
                const openingIntentTokens = { ...current.openingIntentTokens }
                delete openingIntentTokens[descriptorId]
                return {
                    connections,
                    liveMru,
                    activeDescriptorId,
                    activeConnId: projectedConnId(connections, activeDescriptorId),
                    openingIntentTokens,
                    sessions: {
                        ...current.sessions,
                        [descriptorId]: {
                            descriptorId,
                            connId: connection.connId,
                            status: "connected",
                            error: null
                        }
                    }
                }
            })
            await get().loadTables(descriptorId)
            return { outcome: "completed" }
        } catch (error) {
            if (!operationStillCurrent(
                get(), descriptorId, configGeneration, "open", openToken
            )) return { outcome: "cancelled" }
            const code = dbProfileUiErrorCode(error)
            const needsCredential = dbProfileNeedsCredentialPrompt(code)
            set((current) => {
                if (!operationStillCurrent(
                    current, descriptorId, configGeneration, "open", openToken
                )) return {}
                const openingIntentTokens = { ...current.openingIntentTokens }
                delete openingIntentTokens[descriptorId]
                return {
                    openingIntentTokens,
                    sessions: {
                        ...current.sessions,
                        [descriptorId]: {
                            descriptorId,
                            connId: null,
                            status: "error",
                            error: code
                        }
                    },
                    reconnectRequestToken: needsCredential
                        ? current.reconnectRequestToken + 1
                        : current.reconnectRequestToken,
                    reconnectRequest: needsCredential
                        ? {
                              descriptorId,
                              token: current.reconnectRequestToken + 1
                          }
                        : current.reconnectRequest
                }
            })
            // Opening the explicit credential dialog successfully handles these
            // recoverable outcomes. Callers such as the context menu must not
            // show a second generic action-error dialog on top of it.
            return needsCredential ? { outcome: "completed" } : { outcome: "error", error }
        }
    },

    openConfig: async (config) => {
        const intentToken = beginUserIntent(null)
        const target = profileTargetOf(config)
        const credential = config.kind === "sqlite" ? null : { password: config.password }
        try {
            const outcome = await dbProfileCreate({
                name: profileName(target),
                target,
                credential
            })
            const descriptor = savedFromProfile(outcome.profile)
            if (outcome.outcome === "connected") {
                const connection = connectionFromLive(outcome.connection, descriptor)
                set((current) => {
                    const saved = [
                        ...current.saved.filter((item) => item.id !== descriptor.id),
                        descriptor
                    ]
                    const connections = [
                        ...current.connections.filter(
                            (item) => item.descriptorId !== descriptor.id
                        ),
                        connection
                    ]
                    const shouldActivate = current.latestUserIntentToken === intentToken
                    const liveMru = shouldActivate
                        ? touchLiveMru(connections, current.liveMru, descriptor.id)
                        : appendLiveMru(connections, current.liveMru, descriptor.id)
                    const activeDescriptorId = shouldActivate
                        ? descriptor.id
                        : current.activeDescriptorId && connections.some(
                            (item) => item.descriptorId === current.activeDescriptorId
                        )
                            ? current.activeDescriptorId
                            : null
                    const bumped = bumpOperations(current.operations, descriptor.id, "open")
                    return {
                        saved,
                        connections,
                        operations: bumped.operations,
                        historyBuckets: current.historyBuckets[descriptor.id]
                            ? current.historyBuckets
                            : { ...current.historyBuckets, [descriptor.id]: [] },
                        liveMru,
                        activeDescriptorId,
                        activeConnId: projectedConnId(connections, activeDescriptorId),
                        sessions: {
                            ...current.sessions,
                            [descriptor.id]: {
                                descriptorId: descriptor.id,
                                connId: connection.connId,
                                status: "connected",
                                error: null
                            }
                        }
                    }
                })
                await get().loadTables(descriptor.id)
            } else {
                set((current) => ({
                    saved: [
                        ...current.saved.filter((item) => item.id !== descriptor.id),
                        descriptor
                    ],
                    sessions: {
                        ...current.sessions,
                        [descriptor.id]: {
                            descriptorId: descriptor.id,
                            connId: null,
                            status: "error",
                            error: dbProfileUiErrorCode(outcome.error)
                        }
                    },
                    historyBuckets: current.historyBuckets[descriptor.id]
                        ? current.historyBuckets
                        : { ...current.historyBuckets, [descriptor.id]: [] },
                    profileError: "savedButConnectFailed"
                }))
            }
            return outcome
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            try {
                const snapshot = await dbProfileList()
                set((current) => ({
                    ...reconciledSnapshotFields(current, snapshot),
                    profileError: code
                }))
            } catch {
                set({ profileError: code })
            }
            throw error
        }
    },

    openConnection: (path) => get().openConfig({ kind: "sqlite", path }),

    closeConnection: async (connId) => {
        // Resolve the descriptor and delegate to disconnect so a closed connection
        // keeps its saved descriptor and gets a "disconnected" session badge.
        const descriptorId = get().connections.find((c) => c.connId === connId)?.descriptorId
        if (!descriptorId) return
        await get().disconnect(descriptorId)
    },

    disconnect: async (id) => {
        const state = get()
        const connection = liveConnectionForDescriptor(state, id)
        if (!connection) return false
        const configGeneration = savedConfigGeneration(
            state.saved.find((profile) => profile.id === id)
        )
        if (configGeneration === null) return false
        const identity = identityOf(connection)
        if (!identity) {
            set((current) => ({
                sessions: {
                    ...current.sessions,
                    [id]: {
                        descriptorId: id,
                        connId: null,
                        status: "error",
                        error: "staleConnection"
                    }
                }
            }))
            return false
        }
        const disconnectToken = beginOperation(
            id,
            "disconnect",
            ["open", "tables", "columns", "query", "page"]
        )
        beginUserIntent(null)
        try {
            await dbProfileDisconnect(identity)
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            if (!operationStillCurrent(
                get(), id, configGeneration, "disconnect", disconnectToken, identity
            )) return false
            if (code === "serverDisconnected" && identity) {
                projectExactServerDisconnect(identity)
                return false
            }
            set((s) => {
                if (!operationStillCurrent(
                    s, id, configGeneration, "disconnect", disconnectToken, identity
                )) return {}
                const prev = s.sessions[id]
                if (!prev) return {}
                return {
                    sessions: {
                        ...s.sessions,
                        [id]: { ...prev, status: "error", error: code }
                    }
                }
            })
            return false
        }
        set((s) => {
            if (!operationStillCurrent(
                s, id, configGeneration, "disconnect", disconnectToken, identity
            )) return {}
            const prev = s.sessions[id]
            const connections = s.connections.filter((candidate) =>
                !(candidate.descriptorId === identity.descriptorId &&
                    candidate.connId === identity.connectionId &&
                    candidate.connectionGeneration === identity.connectionGeneration)
            )
            const tables = { ...s.tables }
            const queries = { ...s.queries }
            delete tables[connection.connId]
            delete queries[connection.connId]
            const liveMru = liveOnlyMru(
                connections,
                s.liveMru.filter((descriptorId) => descriptorId !== id)
            )
            const activeDescriptorId = s.activeDescriptorId === id
                ? fallbackActiveDescriptor(connections, liveMru)
                : s.activeDescriptorId
            const existingQuery = s.queryBuckets[id]
            return {
                connections,
                tables,
                queries,
                liveMru,
                activeDescriptorId,
                activeConnId: projectedConnId(connections, activeDescriptorId),
                latestUserIntentDescriptorId: activeDescriptorId,
                queryBuckets: existingQuery
                    ? {
                          ...s.queryBuckets,
                          [id]: { ...existingQuery, running: false }
                      }
                    : s.queryBuckets,
                sessions: prev
                    ? {
                          ...s.sessions,
                          [id]: { ...prev, status: "disconnected", connId: null, error: null }
                      }
                    : s.sessions
            }
        })
        return true
    },

    updateSaved: async (id, config) => {
        const previous = get().saved.find((profile) => profile.id === id)
        if (!previous) return
        const configGeneration = savedConfigGeneration(previous)
        if (configGeneration === null) return
        const editToken = beginOperation(
            id,
            "edit",
            ["open", "disconnect", "tables", "columns", "query", "page"]
        )
        beginUserIntent(null)
        try {
            const replacementCredential =
                config.kind !== "sqlite" && config.password.length > 0
                    ? { password: config.password }
                    : null
            const updated = savedFromProfile(
                await dbProfileUpdate({
                    descriptorId: id as DbDescriptorId,
                    name: profileName(profileTargetOf(config)),
                    target: profileTargetOf(config),
                    replacementCredential
                })
            )
            if (!operationStillCurrent(get(), id, configGeneration, "edit", editToken)) return
            const previousTarget = profileFromSaved(previous)?.target
            const invalidatedLive =
                JSON.stringify(previousTarget) !== JSON.stringify(profileTargetOf(config)) ||
                replacementCredential !== null
            set((current) => {
                if (!operationStillCurrent(current, id, configGeneration, "edit", editToken)) {
                    return {}
                }
                const live = liveConnectionForDescriptor(current, id)
                const connections = invalidatedLive
                    ? current.connections.filter((item) => item.descriptorId !== id)
                    : current.connections.map((item) =>
                          item.descriptorId === id
                              ? { ...item, name: updated.name, title: savedConnectionAddress(updated) }
                              : item
                      )
                const tables = { ...current.tables }
                const queries = { ...current.queries }
                if (invalidatedLive && live) {
                    delete tables[live.connId]
                    delete queries[live.connId]
                }
                const tableBuckets = { ...current.tableBuckets }
                const tableErrors = { ...current.tableErrors }
                const columnBuckets = { ...current.columnBuckets }
                const columnErrors = { ...current.columnErrors }
                if (invalidatedLive) {
                    delete tableBuckets[id]
                    delete tableErrors[id]
                    delete columnBuckets[id]
                    delete columnErrors[id]
                }
                const existingQuery = current.queryBuckets[id]
                const queryBuckets = invalidatedLive && existingQuery
                    ? {
                          ...current.queryBuckets,
                          [id]: {
                              ...EMPTY_DB_QUERY,
                              sql: existingQuery.sql
                          }
                      }
                    : current.queryBuckets
                const liveMru = invalidatedLive
                    ? liveOnlyMru(connections, current.liveMru.filter((descriptorId) => descriptorId !== id))
                    : liveOnlyMru(connections, current.liveMru)
                const activeDescriptorId = invalidatedLive && current.activeDescriptorId === id
                    ? fallbackActiveDescriptor(connections, liveMru)
                    : current.activeDescriptorId
                return {
                    saved: current.saved.map((item) => (item.id === id ? updated : item)),
                    connections,
                    tables,
                    queries,
                    tableBuckets,
                    tableErrors,
                    columnBuckets,
                    columnErrors,
                    queryBuckets,
                    liveMru,
                    activeDescriptorId,
                    activeConnId: projectedConnId(connections, activeDescriptorId),
                    latestUserIntentDescriptorId: activeDescriptorId,
                    sessions: {
                        ...current.sessions,
                        [id]: {
                            descriptorId: id,
                            connId: invalidatedLive ? null : (live?.connId ?? null),
                            status: invalidatedLive
                                ? "disconnected"
                                : (current.sessions[id]?.status ?? "disconnected"),
                            error: null
                        }
                    }
                }
            })
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            const snapshot = await bestEffortProfileSnapshot()
            set((current) => {
                if (!operationStillCurrent(current, id, configGeneration, "edit", editToken)) {
                    return { profileError: code }
                }
                const reconciled = snapshot
                    ? reconciledSnapshotFields(current, snapshot)
                    : null
                const baseConnections = reconciled?.connections ?? current.connections
                const removedConnectionIds = new Set(
                    baseConnections
                        .filter((connection) => connection.descriptorId === id)
                        .map((connection) => connection.connId)
                )
                const connections = baseConnections.filter(
                    (connection) => connection.descriptorId !== id
                )
                const tables = Object.fromEntries(
                    Object.entries(reconciled?.tables ?? current.tables).filter(
                        ([connId]) => !removedConnectionIds.has(connId)
                    )
                )
                const queries = Object.fromEntries(
                    Object.entries(reconciled?.queries ?? current.queries).filter(
                        ([connId]) => !removedConnectionIds.has(connId)
                    )
                )
                const saved = reconciled?.saved ?? current.saved
                const sessions = { ...(reconciled?.sessions ?? current.sessions) }
                if (saved.some((profile) => profile.id === id)) {
                    sessions[id] = {
                        descriptorId: id,
                        connId: null,
                        status: "error",
                        error: code
                    }
                } else {
                    delete sessions[id]
                }
                const tableBuckets = { ...(reconciled?.tableBuckets ?? current.tableBuckets) }
                const tableErrors = { ...(reconciled?.tableErrors ?? current.tableErrors) }
                const columnBuckets = { ...(reconciled?.columnBuckets ?? current.columnBuckets) }
                const columnErrors = { ...(reconciled?.columnErrors ?? current.columnErrors) }
                delete tableBuckets[id]
                delete tableErrors[id]
                delete columnBuckets[id]
                delete columnErrors[id]
                const liveMru = liveOnlyMru(
                    connections,
                    (reconciled?.liveMru ?? current.liveMru).filter(
                        (descriptorId) => descriptorId !== id
                    )
                )
                const previousActiveDescriptorId =
                    reconciled?.activeDescriptorId ?? current.activeDescriptorId
                const activeDescriptorId = previousActiveDescriptorId === id
                    ? fallbackActiveDescriptor(connections, liveMru)
                    : previousActiveDescriptorId
                return {
                    ...(reconciled ?? {}),
                    connections,
                    tables,
                    queries,
                    tableBuckets,
                    tableErrors,
                    columnBuckets,
                    columnErrors,
                    liveMru,
                    sessions,
                    activeDescriptorId,
                    activeConnId: projectedConnId(connections, activeDescriptorId),
                    latestUserIntentDescriptorId: activeDescriptorId,
                    profileError: code
                }
            })
            throw error
        }
    },

    removeSaved: async (id) => {
        const configGeneration = savedConfigGeneration(
            get().saved.find((profile) => profile.id === id)
        )
        if (configGeneration === null) return
        const removeToken = beginOperation(
            id,
            "remove",
            ["open", "edit", "disconnect", "tables", "columns", "query", "page"]
        )
        beginUserIntent(null)
        try {
            const snapshot = await dbProfileForget(id as DbDescriptorId)
            set((current) => {
                if (!operationStillCurrent(
                    current, id, configGeneration, "remove", removeToken
                )) return {}
                const reconciled = reconciledSnapshotFields(current, snapshot)
                return {
                    ...reconciled,
                    latestUserIntentDescriptorId: reconciled.activeDescriptorId,
                    profileError: null
                }
            })
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            const snapshot = await bestEffortProfileSnapshot()
            set((current) => {
                if (!operationStillCurrent(
                    current, id, configGeneration, "remove", removeToken
                )) return { profileError: code }
                return snapshot
                    ? { ...reconciledSnapshotFields(current, snapshot), profileError: code }
                    : { profileError: code }
            })
            throw error
        }
    },

    removeCredential: async (id) => {
        const configGeneration = savedConfigGeneration(
            get().saved.find((profile) => profile.id === id)
        )
        if (configGeneration === null) return
        const removeToken = beginOperation(
            id,
            "remove",
            ["open", "edit", "disconnect", "tables", "columns", "query", "page"]
        )
        beginUserIntent(null)
        try {
            const snapshot = await dbProfileRemoveCredential(id as DbDescriptorId)
            set((current) => {
                if (!operationStillCurrent(
                    current, id, configGeneration, "remove", removeToken
                )) return {}
                const reconciled = reconciledSnapshotFields(current, snapshot)
                return {
                    ...reconciled,
                    latestUserIntentDescriptorId: reconciled.activeDescriptorId,
                    profileError: null
                }
            })
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            const snapshot = await bestEffortProfileSnapshot()
            set((current) => {
                if (!operationStillCurrent(
                    current, id, configGeneration, "remove", removeToken
                )) return { profileError: code }
                return snapshot
                    ? { ...reconciledSnapshotFields(current, snapshot), profileError: code }
                    : { profileError: code }
            })
            throw error
        }
    },

    recoverProfile: async (request) => {
        try {
            const snapshot = await dbProfileRecover(request)
            set((current) => ({
                ...reconciledSnapshotFields(current, snapshot),
                profileError: null
            }))
        } catch (error) {
            const code = dbProfileUiErrorCode(error)
            const snapshot = await bestEffortProfileSnapshot()
            set((current) => snapshot
                ? { ...reconciledSnapshotFields(current, snapshot), profileError: code }
                : { profileError: code })
            throw error
        }
    },

    setActiveDescriptor: (descriptorId) => {
        const state = get()
        const connection = liveConnectionForDescriptor(state, descriptorId)
        if (!connection) return
        beginUserIntent(descriptorId)
        set((current) => {
            if (!liveConnectionForDescriptor(current, descriptorId)) return {}
            const liveMru = touchLiveMru(current.connections, current.liveMru, descriptorId)
            return {
                activeDescriptorId: descriptorId,
                activeConnId: projectedConnId(current.connections, descriptorId),
                liveMru
            }
        })
        if (!get().tableBuckets[descriptorId] && !get().tableErrors[descriptorId]) {
            void get().loadTables(descriptorId)
        }
    },

    setActiveConnection: (connId) => {
        const descriptorId = get().connections.find(
            (connection) => connection.connId === connId
        )?.descriptorId
        if (!descriptorId) return
        get().setActiveDescriptor(descriptorId)
    },

    loadTables: async (descriptorOrConnectionId) => {
        const state = get()
        const descriptorId = state.saved.some((profile) => profile.id === descriptorOrConnectionId)
            ? descriptorOrConnectionId
            : state.connections.find(
                (connection) => connection.connId === descriptorOrConnectionId
            )?.descriptorId
        if (!descriptorId) return
        const connection = liveConnectionForDescriptor(state, descriptorId)
        const identity = identityOf(connection ?? undefined)
        const configGeneration = savedConfigGeneration(
            state.saved.find((profile) => profile.id === descriptorId)
        )
        if (!connection || !identity || configGeneration === null) return
        const tablesToken = beginOperation(descriptorId, "tables", ["columns"])
        set((current) => ({
            tableErrors: { ...current.tableErrors, [descriptorId]: null }
        }))
        try {
            const tableList = await dbListTables(identity)
            set((current) => {
                if (!operationStillCurrent(
                    current,
                    descriptorId,
                    configGeneration,
                    "tables",
                    tablesToken,
                    identity
                )) return {}
                return {
                    tableBuckets: {
                        ...current.tableBuckets,
                        [descriptorId]: tableList
                    },
                    tableErrors: { ...current.tableErrors, [descriptorId]: null },
                    tables: { ...current.tables, [connection.connId]: tableList }
                }
            })
        } catch (error) {
            if (!operationStillCurrent(
                get(), descriptorId, configGeneration, "tables", tablesToken, identity
            )) return
            const failure = operationalErrorState(error, "metadataFailed")
            if (failure.code === "serverDisconnected") {
                projectExactServerDisconnect(identity)
                return
            }
            set((current) => {
                if (!operationStillCurrent(
                    current,
                    descriptorId,
                    configGeneration,
                    "tables",
                    tablesToken,
                    identity
                )) return {}
                return {
                    tableErrors: {
                        ...current.tableErrors,
                        [descriptorId]: failure
                    }
                }
            })
        }
    },

    loadColumns: async (descriptorId, table) => {
        const state = get()
        const connection = liveConnectionForDescriptor(state, descriptorId)
        const identity = identityOf(connection ?? undefined)
        const configGeneration = savedConfigGeneration(
            state.saved.find((profile) => profile.id === descriptorId)
        )
        if (!connection || !identity || configGeneration === null) return
        const objectKey = dbObjectRefKey(table)
        // Descriptor-wide invalidations (table refresh, edit, disconnect, etc.)
        // advance this epoch. Individual objects use a separate token so two
        // expanded rows may load concurrently without cancelling each other.
        const columnsEpoch = operationCountersFor(state.operations, descriptorId).columns
        const columnsToken = beginColumnRequest(descriptorId, objectKey)
        set((current) => ({
            columnErrors: {
                ...current.columnErrors,
                [descriptorId]: {
                    ...current.columnErrors[descriptorId],
                    [objectKey]: null
                }
            }
        }))
        try {
            const columns = await dbTableColumns(identity, table)
            set((current) => {
                if (!columnRequestStillCurrent(
                    current,
                    descriptorId,
                    objectKey,
                    configGeneration,
                    columnsEpoch,
                    columnsToken,
                    identity
                )) return {}
                return {
                    columnBuckets: {
                        ...current.columnBuckets,
                        [descriptorId]: {
                            ...current.columnBuckets[descriptorId],
                            [objectKey]: columns
                        }
                    },
                    columnErrors: {
                        ...current.columnErrors,
                        [descriptorId]: {
                            ...current.columnErrors[descriptorId],
                            [objectKey]: null
                        }
                    }
                }
            })
        } catch (error) {
            if (!columnRequestStillCurrent(
                get(),
                descriptorId,
                objectKey,
                configGeneration,
                columnsEpoch,
                columnsToken,
                identity
            )) return
            const failure = operationalErrorState(error, "metadataFailed")
            if (failure.code === "serverDisconnected") {
                projectExactServerDisconnect(identity)
                return
            }
            set((current) => {
                if (!columnRequestStillCurrent(
                    current,
                    descriptorId,
                    objectKey,
                    configGeneration,
                    columnsEpoch,
                    columnsToken,
                    identity
                )) return {}
                return {
                    columnErrors: {
                        ...current.columnErrors,
                        [descriptorId]: {
                            ...current.columnErrors[descriptorId],
                            [objectKey]: failure
                        }
                    }
                }
            })
        } finally {
            finishColumnRequest(descriptorId, objectKey, columnsToken)
        }
    },

    setSql: (sql) =>
        set((state) => {
            const descriptorId = state.activeDescriptorId
            if (!descriptorId) return {}
            const connection = liveConnectionForDescriptor(state, descriptorId)
            if (!connection) return {}
            const next = { ...queryFor(state, descriptorId), sql }
            return {
                queryBuckets: { ...state.queryBuckets, [descriptorId]: next },
                queries: { ...state.queries, [connection.connId]: next }
            }
        }),

    runQuery: async (target) => {
        const state = get()
        const descriptorId = state.activeDescriptorId
        if (!descriptorId) return
        const connection = liveConnectionForDescriptor(state, descriptorId)
        const identity = identityOf(connection ?? undefined)
        const configGeneration = savedConfigGeneration(
            state.saved.find((profile) => profile.id === descriptorId)
        )
        if (!connection || !identity || configGeneration === null) return
        const current = queryFor(state, descriptorId)
        if (current.running) return
        const sql = current.sql
        if (!sql.trim()) return
        const requestedTarget = target ?? { kind: "all" as const }
        const resolved = resolveDatabaseSqlTarget(sql, connection.kind, requestedTarget)
        if (!resolved.ok) {
            set((snapshot) => {
                if (!exactConnection(snapshot, identity)) return {}
                const next = {
                    ...queryFor(snapshot, descriptorId),
                    running: false,
                    parseError: resolved.error
                }
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: { ...snapshot.queries, [connection.connId]: next }
                }
            })
            return
        }
        const queryToken = beginOperation(descriptorId, "query", ["page"])
        const queryRunId = `${descriptorId}:query:${queryToken}` as DbQueryRunId
        const owner: DbQueryRunOwner = { ...identity, queryRunId }
        const mode = requestedTarget.kind === "all" || resolved.units.length > 1 ? "script" : "primary"
        let startedAt = performance.now()
        const previousSessions = current.runGroup?.run
            ? resultSessionOwners(current.runGroup.run)
            : []
        const executedSql = resolved.units.map((unit) => unit.sql).join("\n")
        set((snapshot) => {
            if (!operationStillCurrent(
                snapshot, descriptorId, configGeneration, "query", queryToken, identity
            )) return {}
            const next: DbQueryState = {
                ...queryFor(snapshot, descriptorId),
                running: true,
                error: null,
                parseError: null
            }
            return {
                queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                queries: { ...snapshot.queries, [connection.connId]: next }
            }
        })
        try {
            for (const sessionOwner of previousSessions) {
                try {
                    await dbResultSessionRelease(sessionOwner)
                } catch (error) {
                    if (operationalErrorState(error, "queryFailed").code !== "staleConnection") throw error
                }
                if (!operationStillCurrent(
                    get(), descriptorId, configGeneration, "query", queryToken, identity
                )) return
            }
            startedAt = performance.now()
            set((snapshot) => {
                if (!operationStillCurrent(
                    snapshot, descriptorId, configGeneration, "query", queryToken, identity
                )) return {}
                const next: DbQueryState = {
                    ...queryFor(snapshot, descriptorId),
                    sortBy: null,
                    sortBaseRows: null,
                    runGroup: {
                        owner,
                        mode,
                        units: [...resolved.units],
                        status: "running",
                        run: null,
                        activeStatementExecutionId: null,
                        resultPages: {},
                        startedAt,
                        cancelOutcome: null
                    }
                }
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: { ...snapshot.queries, [connection.connId]: next }
                }
            })
            const run = await dbQueryRun({
                ...owner,
                mode,
                statements: resolved.units.map((unit) => ({
                    sql: unit.sql,
                    transactionBoundary: unit.transactionBoundary
                })) as [
                    { sql: string; transactionBoundary: DatabaseSqlUnit["transactionBoundary"] },
                    ...Array<{ sql: string; transactionBoundary: DatabaseSqlUnit["transactionBoundary"] }>
                ]
            })
            if (!queryRunMatchesOwner(run, owner)) {
                throw { code: "staleConnection", message: "query run owner mismatch" }
            }
            const elapsedMs = Math.round(performance.now() - startedAt)
            let published = false
            set((snapshot) => {
                if (!operationStillCurrent(
                    snapshot, descriptorId, configGeneration, "query", queryToken, identity
                )) return {}
                published = true
                const statement = preferredStatement(run)
                const resultPages = resultPageStatesForRun(run)
                const resultSession = resultSessionForStatement(statement)
                const pageState = resultSession
                    ? resultPages[resultPageKey(resultSession.owner)] ?? null
                    : null
                const projection = legacyProjectionForStatement(statement, pageState)
                const existing = queryFor(snapshot, descriptorId)
                const next: DbQueryState = {
                    ...existing,
                    running: false,
                    result: projection.result,
                    error: projection.error,
                    elapsedMs,
                    lastSql: projection.lastSql,
                    sortBy: null,
                    sortBaseRows: null,
                    parseError: null,
                    runGroup: existing.runGroup?.owner.queryRunId === queryRunId
                        ? {
                              ...existing.runGroup,
                              status: "settled",
                              run,
                              activeStatementExecutionId: statement.statementExecutionId,
                              resultPages
                          }
                        : existing.runGroup
                }
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: { ...snapshot.queries, [connection.connId]: next }
                }
            })
            if (!published) return
            const failedStatement = run.statements.find((statement) =>
                statement.result.kind === "error" ||
                statement.result.kind === "cancelled" ||
                statement.result.kind === "resultLimitReached"
            )
            get().recordHistory(descriptorId, {
                sql: executedSql,
                ranAt: Date.now(),
                ok: !failedStatement,
                error: failedStatement?.result.kind,
                elapsedMs
            })
            const cancelTerminated = queryFor(get(), descriptorId).runGroup?.cancelOutcome ===
                "cancelledConnectionTerminated"
            if (run.connectionTerminated || cancelTerminated) {
                projectExactServerDisconnect(identity, undefined, null)
            } else if (run.statements.some((statement) => statement.result.kind === "execute")) {
                void get().loadTables(descriptorId)
            }
        } catch (error) {
            if (!operationStillCurrent(
                get(), descriptorId, configGeneration, "query", queryToken, identity
            )) return
            const elapsedMs = Math.round(performance.now() - startedAt)
            const failure: DbQueryErrorState = {
                ...operationalErrorState(error, "queryFailed"),
                executedSql
            }
            if (failure.code === "serverDisconnected") {
                projectExactServerDisconnect(identity, failure)
            } else {
                set((snapshot) => {
                    if (!operationStillCurrent(
                        snapshot, descriptorId, configGeneration, "query", queryToken, identity
                    )) return {}
                    const next: DbQueryState = {
                        ...queryFor(snapshot, descriptorId),
                        running: false,
                        result: null,
                        error: failure,
                        elapsedMs,
                        lastSql: null,
                        sortBy: null,
                        sortBaseRows: null,
                        runGroup: queryFor(snapshot, descriptorId).runGroup?.owner.queryRunId === queryRunId
                            ? {
                                  ...queryFor(snapshot, descriptorId).runGroup!,
                                  status: "settled"
                              }
                            : queryFor(snapshot, descriptorId).runGroup
                    }
                    return {
                        queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                        queries: { ...snapshot.queries, [connection.connId]: next }
                    }
                })
            }
            get().recordHistory(descriptorId, {
                sql: executedSql,
                ranAt: Date.now(),
                ok: false,
                error: failure.code,
                elapsedMs
            })
        }
    },

    cancelQuery: async () => {
        const state = get()
        const descriptorId = state.activeDescriptorId
        if (!descriptorId) return
        const connection = liveConnectionForDescriptor(state, descriptorId)
        const identity = identityOf(connection ?? undefined)
        const current = queryFor(state, descriptorId)
        const group = current.runGroup
        if (!connection || !identity || !group || !queryRunGroupIsCancellable(group)) return
        if (
            group.owner.descriptorId !== identity.descriptorId ||
            group.owner.connectionId !== identity.connectionId ||
            group.owner.connectionGeneration !== identity.connectionGeneration
        ) return
        const cancelOwner = group.owner
        const streamingOwner = activeStreamingResultOwner(group)

        set((snapshot) => {
            const nextCurrent = queryFor(snapshot, descriptorId)
            if (
                !nextCurrent.runGroup ||
                !queryRunOwnerMatches(nextCurrent.runGroup.owner, cancelOwner)
            ) return {}
            const next = {
                ...nextCurrent,
                error: null,
                runGroup: { ...nextCurrent.runGroup, status: "cancelling" as const }
            }
            return {
                queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                queries: { ...snapshot.queries, [connection.connId]: next }
            }
        })

        try {
            const result = await dbQueryCancel(cancelOwner)
            if (streamingOwner) {
                currentResultPageRequests.delete(resultPageKey(streamingOwner))
            }
            let settled = false
            set((snapshot) => {
                if (!exactConnection(snapshot, identity)) return {}
                let nextCurrent = queryFor(snapshot, descriptorId)
                if (
                    !nextCurrent.runGroup ||
                    !queryRunOwnerMatches(nextCurrent.runGroup.owner, cancelOwner)
                ) return {}
                if (streamingOwner) {
                    const exactPage = exactResultPageState(nextCurrent, streamingOwner)
                    if (exactPage) {
                        const pageUpdated = queryWithResultPageState(nextCurrent, streamingOwner, {
                            ...exactPage.state,
                            page: {
                                ...exactPage.state.page,
                                lifecycle: "cancelled",
                                hasNext: false
                            },
                            loading: false
                        })
                        if (pageUpdated) nextCurrent = pageUpdated
                    }
                }
                const currentGroup = nextCurrent.runGroup
                if (!currentGroup || !queryRunOwnerMatches(currentGroup.owner, cancelOwner)) return {}
                settled = Boolean(streamingOwner) || currentGroup.status === "settled"
                const next: DbQueryState = {
                    ...nextCurrent,
                    runGroup: {
                        ...currentGroup,
                        cancelOutcome: result.outcome,
                        status: streamingOwner ? "settled" : currentGroup.status
                    }
                }
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: { ...snapshot.queries, [connection.connId]: next }
                }
            })
            if (result.outcome === "cancelledConnectionTerminated" && settled) {
                projectExactServerDisconnect(identity, undefined, null)
            }
        } catch (error) {
            const failure: DbQueryErrorState = {
                ...operationalErrorState(error, "queryFailed"),
                executedSql: group.units.map((unit) => unit.sql).join("\n")
            }
            set((snapshot) => {
                if (!exactConnection(snapshot, identity)) return {}
                const nextCurrent = queryFor(snapshot, descriptorId)
                if (
                    !nextCurrent.runGroup ||
                    !queryRunOwnerMatches(nextCurrent.runGroup.owner, cancelOwner)
                ) return {}
                const next = {
                    ...nextCurrent,
                    error: failure,
                    runGroup: {
                        ...nextCurrent.runGroup,
                        status: nextCurrent.runGroup.run ? "settled" as const : "running" as const
                    }
                }
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: { ...snapshot.queries, [connection.connId]: next }
                }
            })
        }
    },

    selectStatementTab: (statementExecutionId) =>
        set((snapshot) => {
            let descriptorId = snapshot.activeDescriptorId
            let current = descriptorId ? queryFor(snapshot, descriptorId) : null
            let statement = current?.runGroup?.run?.statements.find((candidate) =>
                candidate.statementExecutionId === statementExecutionId
            )
            if (!descriptorId || !current || !statement) {
                const offlineEntry = Object.entries(snapshot.queryBuckets).find(([, query]) =>
                    query.runGroup?.run?.statements.some((candidate) =>
                        candidate.statementExecutionId === statementExecutionId
                    )
                )
                if (!offlineEntry) return {}
                descriptorId = offlineEntry[0]
                current = offlineEntry[1]
                statement = current.runGroup!.run!.statements.find((candidate) =>
                    candidate.statementExecutionId === statementExecutionId
                )
            }
            if (!descriptorId || !current?.runGroup || !statement) return {}
            const pageState = resultPageStateForStatement(current.runGroup, statement)
            const projection = legacyProjectionForStatement(statement, pageState)
            const next: DbQueryState = {
                ...current,
                result: projection.result,
                error: projection.error,
                lastSql: projection.lastSql,
                sortBy: pageState?.sort ?? null,
                sortBaseRows: pageState?.sort ? pageState.sortBaseRows : null,
                runGroup: { ...current.runGroup, activeStatementExecutionId: statementExecutionId }
            }
            const connection = liveConnectionForDescriptor(snapshot, descriptorId)
            return {
                queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                queries: connection
                    ? { ...snapshot.queries, [connection.connId]: next }
                    : snapshot.queries
            }
        }),

    previousResultPage: (owner) => requestResultPage(owner, "previous"),

    nextResultPage: (owner) => requestResultPage(owner, "next"),

    releaseResultSession: (owner) => requestResultPage(owner, "release"),

    sortResult: async (columnIndex, owner) => {
        const state = get()
        const descriptorId = owner?.descriptorId ?? state.activeDescriptorId
        if (!descriptorId) return
        const current = queryFor(state, descriptorId)
        const group = current.runGroup
        if (group?.run) {
            const statement = owner
                ? exactResultPageState(current, owner)?.statement ?? null
                : group.run.statements.find((candidate) =>
                      candidate.statementExecutionId === group.activeStatementExecutionId
                  ) ?? null
            const session = statement ? resultSessionForStatement(statement) : null
            const targetOwner = owner ?? session?.owner ?? null
            const exact = targetOwner ? exactResultPageState(current, targetOwner) : null
            if (!targetOwner || !exact || exact.state.loading) return
            const prev = exact.state.sort
            const nextSort: DbSort | null =
                !prev || prev.columnIndex !== columnIndex
                    ? { columnIndex, dir: "asc" }
                    : prev.dir === "asc"
                      ? { columnIndex, dir: "desc" }
                      : null
            const rows = sortedPageRows(exact.state.sortBaseRows, nextSort)
            set((snapshot) => {
                const latest = queryFor(snapshot, descriptorId)
                const latestPage = exactResultPageState(latest, targetOwner)
                if (!latestPage || latestPage.state.loading) return {}
                const next = queryWithResultPageState(latest, targetOwner, {
                    ...latestPage.state,
                    page: { ...latestPage.state.page, rows },
                    sort: nextSort
                })
                if (!next) return {}
                const connection = liveConnectionForDescriptor(snapshot, descriptorId)
                return {
                    queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                    queries: connection
                        ? { ...snapshot.queries, [connection.connId]: next }
                        : snapshot.queries
                }
            })
            return
        }

        const connection = liveConnectionForDescriptor(state, descriptorId)
        if (!connection) return
        if (current.running || current.result?.kind !== "select") return
        const prev = current.sortBy
        const nextSort: DbSort | null =
            !prev || prev.columnIndex !== columnIndex
                ? { columnIndex, dir: "asc" }
                : prev.dir === "asc"
                  ? { columnIndex, dir: "desc" }
                  : null
        const baseRows = current.sortBaseRows ?? current.result.rows
        const rows = nextSort
            ? [...baseRows].sort((left, right) => {
                  const order = compareDbValues(left[columnIndex], right[columnIndex])
                  return nextSort.dir === "asc" ? order : -order
              })
            : baseRows
        set((snapshot) => {
            const latest = queryFor(snapshot, descriptorId)
            if (latest.running || latest.result?.kind !== "select") return {}
            const next: DbQueryState = {
                ...latest,
                result: { ...latest.result, rows },
                sortBy: nextSort,
                sortBaseRows: nextSort ? baseRows : null
            }
            return {
                queryBuckets: { ...snapshot.queryBuckets, [descriptorId]: next },
                queries: { ...snapshot.queries, [connection.connId]: next }
            }
        })
    },

    openTableQuery: async (table) => {
        const descriptorId = get().activeDescriptorId
        if (!descriptorId) return
        const kind = liveConnectionForDescriptor(get(), descriptorId)?.kind ?? "sqlite"
        get().setSql(buildTableQuery(kind, table))
        await get().runQuery()
    },

    recordHistory: (descriptorId, entry) =>
        set((state) => {
            const sql =
                entry.sql.length > DB_HISTORY_SQL_MAX ? entry.sql.slice(0, DB_HISTORY_SQL_MAX) : entry.sql
            const existing = state.historyBuckets[descriptorId] ?? []
            // A consecutive re-run of the identical statement doesn't earn a new row.
            if (existing[0]?.sql === sql) return {}
            const next = [{ ...entry, sql }, ...existing].slice(0, DB_HISTORY_LIMIT)
            const historyBuckets = { ...state.historyBuckets, [descriptorId]: next }
            return { historyBuckets }
        }),

    reset: () => {
        pendingLegacyHistoryCleanup = hasLegacyHistoryStorageKey()
        currentColumnRequests.clear()
        currentResultPageRequests.clear()
        set({ ...dbInitialState, saved: loadSavedConnections() })
    }
    })
})
