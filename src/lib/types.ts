export interface FileNode {
    name: string
    path: string
    isDir: boolean
}

export interface WorkspacePathIndexEntry {
    relativePath: string
    canonicalPath: string
}

export interface WorkspacePathIndexResult {
    workspace: string
    entries: WorkspacePathIndexEntry[]
    truncated: boolean
}

export type DocumentLineEnding = "lf" | "crlf" | "mixed"

export type OpenFileResult =
    | { kind: "full"; content: string; size: number; lineEnding: DocumentLineEnding }
    | { kind: "limited"; content: string; size: number; lineEnding: DocumentLineEnding }
    | { kind: "tooLarge"; size: number }
    | { kind: "binary"; size: number }
    | { kind: "nonUtf8Readonly"; content: string; encoding: string; size: number }

const LANGUAGE_BY_EXT: Record<string, string> = {
    ts: "TypeScript",
    tsx: "TypeScript",
    js: "JavaScript",
    jsx: "JavaScript",
    py: "Python",
    rs: "Rust",
    md: "Markdown",
    json: "JSON",
    html: "HTML",
    css: "CSS",
    toml: "TOML",
    yaml: "YAML",
    yml: "YAML"
}

export function languageFromPath(path: string): string {
    const ext = path.split(".").pop()?.toLowerCase() ?? ""
    return LANGUAGE_BY_EXT[ext] ?? "Plain Text"
}

export const MAX_LINE_LEN_SYNTAX_OFF = 10_000

// --- LSP (T4 serde contract; all outputs camelCase) ---
export type LspLanguage = "typescript" | "python" | "rust" | "markdown"

// Maps a file path to its LSP language, or null when Yuzora ships no server for
// it. Lives here (not in lspManager) so lspStore can use it without importing
// the client-lifecycle module (avoids a store <-> manager import cycle).
const LSP_LANGUAGE_BY_EXT: Record<string, LspLanguage> = {
    ts: "typescript",
    tsx: "typescript",
    mts: "typescript",
    cts: "typescript",
    js: "typescript",
    jsx: "typescript",
    mjs: "typescript",
    cjs: "typescript",
    py: "python",
    pyi: "python",
    rs: "rust",
    md: "markdown",
    markdown: "markdown"
}

export function lspLanguageOf(path: string): LspLanguage | null {
    const ext = path.split(".").pop()?.toLowerCase() ?? ""
    return LSP_LANGUAGE_BY_EXT[ext] ?? null
}
export type LspProcessStatus =
    | { status: "starting" }
    | { status: "missing"; installHint: string }
    | { status: "crashed"; reason: string }
    | { status: "stopped" }
export interface LspServerInfo {
    // Raw workspace string this info belongs to (the Rust-side process-map key).
    // Used to drop stale events from a workspace the UI has already left.
    workspace: string
    language: string
    serverId: string
    command: string
    path: string | null
    status: LspProcessStatus
    lastStartupLog: string | null
    lastError: string | null
    restartCount: number
}
export interface LspConfig {
    defaults: Record<string, string>
    workspaces: Record<string, Record<string, string>>
}
// 狀態列顯示態（前端組合 process status＋client initialized＋檔案分級推導）
export type LspDisplayState = "ready" | "starting" | "failed" | "missing" | "syntaxOnly"
// 一鍵安裝進度（T14 emit "lsp:install-progress"；T12 listen 顯示）
export interface LspInstallProgress {
    language: string
    phase: "download" | "verify" | "unpack" | "npm" | "pip" | "done" | "error"
    percent: number | null
    message: string | null
}
// 檔案分級（LSP 掛載判準；由 OpenFileResult.kind＋hasVeryLongLine 推導）
export type FileGrade = "full" | "limited" | "tooLarge" | "binary" | "nonUtf8Readonly" | "veryLongLine"

function hasVeryLongLine(content: string): boolean {
    for (const line of content.split("\n")) {
        if (line.length > MAX_LINE_LEN_SYNTAX_OFF) return true
    }
    return false
}

export function fileGradeOf(result: OpenFileResult, content?: string): FileGrade {
    if (result.kind === "full" && hasVeryLongLine(content ?? result.content)) return "veryLongLine"
    return result.kind
}

export interface GitFileEntry { path: string; origPath: string | null; status: string }
export interface GitStatus {
    branch: string | null
    headOid: string
    detached: boolean
    upstream: string | null
    ahead: number
    behind: number
    staged: GitFileEntry[]
    unstaged: GitFileEntry[]
    untracked: string[]
    conflicted: GitFileEntry[]
    inProgress: string | null
}
export type GitEnvironment =
    | { status: "missing"; reason: string }
    | { status: "notARepo" }
    | { status: "ready"; root: string; version: string }
export interface BranchInfo { name: string; upstream: string | null; ahead: number; behind: number; isCurrent: boolean }
export interface BranchList { local: BranchInfo[]; remote: string[] }
export type RemoteProbe = "yes" | "no" | "unknown"
export type GradedText =
    | { kind: "full"; content: string }
    | { kind: "limited"; content: string }
    | { kind: "tooLarge" }
    | { kind: "binary" }
export interface DiffContent { original: GradedText; modified: GradedText }
export interface SearchMatch { line: number; col: number; preview: string }
export type SearchEvent =
    | { type: "match"; path: string; matches: SearchMatch[] }
    | { type: "done"; truncated: boolean; fileCount: number }
export interface PtySessionInfo {
    sessionId: string
    workspace: string
    shell: string
    cols: number
    rows: number
}
export type PtyEvent =
    | { type: "output"; data: string }
    | { type: "exit"; code: number | null }
export interface DevServerCandidate {
    scriptName: string
    command: string
    likelyPort: number | null
}
export interface DevServerDetect {
    candidates: DevServerCandidate[]
    runningPorts: number[]
}
export type DevServerStatus =
    | { status: "starting" }
    | { status: "running"; port: number | null }
    | { status: "exited"; code: number | null }
    | { status: "failed"; reason: string }
export interface DevServerInfo {
    workspace: string
    command: string
    port: number | null
    status: DevServerStatus
}
export type AskpassKind = "username" | "password" | "passphrase" | "fingerprint" | "other"
export interface AskpassRequest { id: number; prompt: string; kind: AskpassKind }

// --- Runtime logs (M5 Task 15; Rust serde field names are snake_case) ---
export interface LogRecord {
    timestamp: string
    level: string
    kind: string
    source: string
    workspace_path: string | null
    event: string
    message: string
    metadata: unknown
}

// --- Git log (T2 IPC contract; all outputs camelCase) ---
export type LogRefKind = "head" | "local" | "remote" | "tag"
export interface LogRef { name: string; kind: LogRefKind }
export interface LogCommit {
    hash: string
    shortHash: string
    subject: string
    authorName: string
    authorEmail: string
    timestamp: number
    parents: string[]
    refs: LogRef[]
}
export interface LogPage { commits: LogCommit[]; hasMore: boolean }
export interface CommitFileChange {
    status: string
    path: string
    oldPath: string | null
    additions: number
    deletions: number
    binary: boolean
}
export interface CommitDetail {
    subject: string
    body: string
    authorName: string
    authorEmail: string
    timestamp: number
    parents: string[]
    files: CommitFileChange[]
    totalAdditions: number
    totalDeletions: number
}
export interface AuthorEntry { name: string; email: string }
export type FileAtRevResult =
    | { kind: "full"; content: string }
    | { kind: "limited"; content: string }
    | { kind: "tooLarge" }
    | { kind: "binary" }
    | { kind: "missing" }

// --- Database (FEAT-1 SQLite + F2 network backends; Rust serde is camelCase) ---
export type DbKind = "sqlite" | "postgres" | "mssql"
// Write-only connection input used behind the Rust profile/Test Connection
// authority. No renderer-facing command can register this config directly;
// passwords are sent in-flight only and are NEVER persisted anywhere.
export type DbOpenConfig =
    | { kind: "sqlite"; path: string }
    | {
          kind: "postgres"
          host: string
          port: number
          database: string
          user: string
          password: string
          ssl: boolean
          trustCert: boolean
      }
    | {
          kind: "mssql"
          host: string
          port: number
          database: string
          user: string
          password: string
          trustCert: boolean
      }

declare const dbOpaqueId: unique symbol

/** IDs are deliberately content-independent. A host/path/database tuple is a
 * target address, never the identity of a saved profile or live operation. */
export type DbOpaqueId<Kind extends string> = string & {
    readonly [dbOpaqueId]: Kind
}
export type DbDescriptorId = DbOpaqueId<"descriptor">
/** Product terminology uses profile and descriptor for the same saved entity. */
export type DbProfileId = DbDescriptorId
export type DbConnectionId = DbOpaqueId<"connection">
export type DbConnectionGeneration = DbOpaqueId<"connectionGeneration">
export type DbQueryRunId = DbOpaqueId<"queryRun">
export type DbStatementExecutionId = DbOpaqueId<"statementExecution">
export type DbResultSessionId = DbOpaqueId<"resultSession">

/** Non-secret connection address. Passwords are accepted only by write-only
 * request contracts and can never appear in a returned descriptor. */
export type DbProfileTarget =
    | { kind: "sqlite"; path: string }
    | {
          kind: "postgres"
          host: string
          port: number
          database: string
          user: string
          ssl: boolean
          trustCert: boolean
      }
    | {
          kind: "mssql"
          host: string
          port: number
          database: string
          user: string
          trustCert: boolean
      }

export type DbCredentialState = "notRequired" | "stored" | "required" | "unavailable"
export interface DbProfileDescriptor {
    descriptorId: DbDescriptorId
    /** Monotonic non-secret configuration revision owned by the Rust profile
     * repository. Async work must revalidate it before publishing results. */
    configGeneration: number
    name: string
    target: DbProfileTarget
    credentialState: DbCredentialState
}
export type DbProfileErrorCode =
    | "repositoryUnavailable"
    | "vaultMissing"
    | "vaultDenied"
    | "vaultUnavailable"
    | "vaultCorrupt"
    | "vaultWriteFailed"
    | "vaultDeleteFailed"
    | "profileNotFound"
    | "pendingOperationConflict"
    | "recoveryNotFound"
    | "recoveryActionInvalid"
    | "credentialRequired"
    | "lifecycleCancelFailed"
    | "lifecycleCloseFailed"
    | "connectionFailed"
    | "connectionBusy"
    | "serverDisconnected"
    | "metadataFailed"
    | "queryFailed"
    | "staleConnection"
    | "sqlitePathMissing"
    | "sqlitePathNotFile"
    | "sqlitePathUnreadable"
    | "sqlitePathInvalid"
    | "sqliteOpenFailed"
    | "invalidRequest"
export interface DbProfileError {
    code: DbProfileErrorCode
    message: string
}
export interface DbCredentialInput { password: string }
export interface DbProfileCreateRequest {
    name: string
    target: DbProfileTarget
    credential: DbCredentialInput | null
}
export interface DbProfileUpdateRequest {
    descriptorId: DbDescriptorId
    name: string
    target: DbProfileTarget
    replacementCredential: DbCredentialInput | null
}
export type DbProfileRecoveryKind =
    | "pendingCreate"
    | "pendingReplace"
    | "cleanupOld"
    | "pendingForget"
    | "pendingRemoveCredential"
export type DbProfileRecoveryAction = "resume" | "abort" | "retryCleanup"
export interface DbProfileRecoveryRow {
    operationId: string
    descriptorId: DbDescriptorId
    kind: DbProfileRecoveryKind
    allowedActions: DbProfileRecoveryAction[]
}
export interface DbProfileLoadResult {
    profiles: DbProfileDescriptor[]
    recovery: DbProfileRecoveryRow[]
}
export interface DbLegacyProfileImportRequest {
    profiles: DbProfileDescriptor[]
}
export interface DbProfileRecoveryRequest {
    operationId: string
    action: DbProfileRecoveryAction
    credential: DbCredentialInput | null
}

export interface DbConnectionIdentity {
    descriptorId: DbDescriptorId
    connectionId: DbConnectionId
    connectionGeneration: DbConnectionGeneration
}
export type DbLiveEngine = DbKind
export interface DbLiveConnection extends DbConnectionIdentity { engine: DbLiveEngine }
export type DbSaveAndConnectOutcome =
    | {
          outcome: "connected"
          profile: DbProfileDescriptor
          connection: DbLiveConnection
      }
    | {
          outcome: "savedButConnectFailed"
          profile: DbProfileDescriptor
          error: DbProfileError
      }

export type DbTestConnectionRequest =
    | { kind: "ephemeral"; target: DbProfileTarget; credential: DbCredentialInput | null }
    | { kind: "saved"; descriptorId: DbDescriptorId }
export interface DbTestConnectionResult {
    elapsedMs: number
    serverVersion: string | null
}

/** Schema-qualified object reference used as the stable identity everywhere
 * outside the legacy P1 UI path. All fields are present for every engine. */
export interface DbObjectReference {
    catalog: string
    schema: string
    name: string
    kind: "table" | "view"
}
export type DbTable = DbObjectReference
export interface DbColumn { name: string; type: string; notnull: boolean; pk: boolean }

/** Every cell is tagged. Integers/decimals use strings so JavaScript never
 * rounds a 64-bit integer or an engine decimal; binary bytes use lossless hex. */
export type DbValue =
    | { kind: "null" }
    | { kind: "boolean"; value: boolean }
    | {
          kind: "integer" | "decimal" | "text" | "json" | "date" | "time" | "dateTime"
          value: string
      }
    | { kind: "binary"; hex: string }

export function formatDbValue(value: DbValue): string | null {
    switch (value.kind) {
        case "null":
            return null
        case "boolean":
            return String(value.value)
        case "binary":
            return `<blob ${value.hex.length / 2} bytes>`
        default:
            return value.value
    }
}

export type DbRetryability = "retryable" | "notRetryable" | "unknown"
export type DbErrorEngine = DbKind | "yuzora"
export interface DbErrorPosition {
    offset: number | null
    line: number | null
    column: number | null
}
export interface DbError {
    engine: DbErrorEngine
    message: string
    code: string | null
    position: DbErrorPosition | null
    detail: string | null
    hint: string | null
    retryability: DbRetryability
}

export type DbOperationalErrorCode =
    | "connectionFailed"
    | "connectionBusy"
    | "serverDisconnected"
    | "metadataFailed"
    | "queryFailed"
    | "staleConnection"
    | "sqlitePathMissing"
    | "sqlitePathNotFile"
    | "sqlitePathUnreadable"
    | "sqlitePathInvalid"
    | "sqliteOpenFailed"

/** Stable recovery code plus optional engine diagnostics. The backend omits
 * `error` when no engine evidence exists. */
export interface DbOperationalError {
    code: DbOperationalErrorCode
    message: string
    error?: DbError | null
}

export type DbEffectOutcome =
    | "none"
    | "committed"
    | "rolledBack"
    | "transactionPending"
    | "unknown"

export type DbQueryResult =
    | {
          kind: "select"
          columns: string[]
          rows: DbValue[][]
          truncated: boolean
          affectedRows: string | null
          effectOutcome: DbEffectOutcome
      }
    | { kind: "execute"; affectedRows: string | null; effectOutcome: DbEffectOutcome }

export interface DbQueryRunOwner extends DbConnectionIdentity { queryRunId: DbQueryRunId }
export interface DbStatementExecutionOwner extends DbQueryRunOwner {
    statementExecutionId: DbStatementExecutionId
}
export interface DbResultSessionOwner extends DbStatementExecutionOwner {
    resultSessionId: DbResultSessionId
}

export interface DbResultSession {
    owner: DbResultSessionOwner
    columns: string[]
    initialPage: DbResultPage
}
export type DbStatementExecutionResult =
    | {
          kind: "rows"
          resultSession: DbResultSession | null
          affectedRows: string | null
      }
    | { kind: "execute"; affectedRows: string | null }
    | { kind: "error"; error: DbError }
    | { kind: "cancelled"; error: DbError }
    | {
          kind: "resultLimitReached"
          resultSession: DbResultSession
          affectedRows: string | null
      }
    | { kind: "skipped" }
export interface DbStatementExecution {
    statementExecutionId: DbStatementExecutionId
    statementIndex: number
    sql: string
    effectOutcome: DbEffectOutcome
    result: DbStatementExecutionResult
}
export type DbNonEmptyArray<T> = readonly [T, ...T[]]
export interface DbQueryRun extends DbQueryRunOwner {
    statements: DbNonEmptyArray<DbStatementExecution>
    transactionMayBeOpen: boolean
    connectionTerminated: boolean
}

export type DbQueryRunMode = "primary" | "script"
export type DbTransactionBoundary = "none" | "begin" | "commit" | "rollback"
export interface DbQueryRunStatement {
    sql: string
    transactionBoundary: DbTransactionBoundary
}
export interface DbQueryRunRequest extends DbConnectionIdentity {
    queryRunId: DbQueryRunId
    mode: DbQueryRunMode
    statements: DbNonEmptyArray<DbQueryRunStatement>
}
export interface DbQueryCancelResult {
    outcome: "cancelled" | "cancelledConnectionTerminated" | "alreadyRequested"
}
export type DbResultPageDirection = "previous" | "next"
export type DbResultSessionLifecycle =
    | "streaming"
    | "complete"
    | "released"
    | "cancelled"
    | "error"
export interface DbResultPageRequest {
    owner: DbResultSessionOwner
    direction: DbResultPageDirection
}
export interface DbResultPage {
    owner: DbResultSessionOwner
    pageIndex: number
    columns: string[]
    rows: DbValue[][]
    hasPrevious: boolean
    hasNext: boolean
    effectOutcome: DbEffectOutcome
    lifecycle: DbResultSessionLifecycle
    resultLimitReached: boolean
}

// --- SSH terminal (FEAT-2 MVP; Rust serde outputs camelCase) ---
export type SshAuthKind = "password" | "key"
// The auth secret sent to ssh_connect. NEVER persisted — password is prompted
// per connection, passphrase (optional) is entered at connect time.
export type SshAuthInput =
    | { kind: "password"; password: string }
    | { kind: "key"; keyPath: string; passphrase?: string }
export interface SshConnectResult { sessionId: string; fingerprint: string; knownHost?: boolean }
export interface SshDataEvent { sessionId: string; chunk: string }
export interface SshExitEvent { sessionId: string }

// --- SFTP browsing + transfers (F5; Rust serde outputs camelCase) ---
export interface SftpEntry {
    name: string
    path: string
    isDir: boolean
    isSymlink: boolean
    size: number
}
export interface SftpListing { cwd: string; entries: SftpEntry[] }
// Progress tick on `sftp://progress`, correlated by the front-end's transferId.
export interface SftpProgressEvent {
    sessionId: string
    transferId: string
    transferred: number
    total: number
    done: boolean
}

// --- Performance monitor (F1; Rust serde outputs camelCase) ---
// The app's own main process. cpuPercent is sysinfo's raw value (can exceed 100
// on multi-core machines); memoryBytes is resident bytes.
export interface PerfSnapshot { cpuPercent: number; memoryBytes: number }
