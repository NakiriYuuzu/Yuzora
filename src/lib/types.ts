export interface FileNode {
    name: string
    path: string
    isDir: boolean
}

export type OpenFileResult =
    | { kind: "full"; content: string; size: number }
    | { kind: "limited"; content: string; size: number }
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
export type AskpassKind = "username" | "password" | "passphrase" | "fingerprint" | "other"
export interface AskpassRequest { id: number; prompt: string; kind: AskpassKind }

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
