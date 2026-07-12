import { Channel, invoke } from "@tauri-apps/api/core"

// Re-exported so feature modules that carry their own domain logic around a
// command (agent/ACP protocol handlers, the log-event envelope builder) can
// reach the IPC boundary through this module instead of importing the Tauri
// core directly. `@tauri-apps/api/core` should be imported only here and in
// `platform.ts` (which owns `isTauri`).
export { invoke }

import type {
    FileNode,
    WorkspacePathIndexResult,
    OpenFileResult,
    GitEnvironment,
    GitStatus,
    BranchList,
    RemoteProbe,
    DiffContent,
    SearchEvent,
    LogPage,
    CommitDetail,
    AuthorEntry,
    FileAtRevResult,
    LspServerInfo,
    LspConfig,
    PtyEvent,
    PtySessionInfo,
    DevServerDetect,
    DevServerInfo,
    DbTable,
    DbColumn,
    DbQueryResult,
    DbOpenConfig,
    SshAuthInput,
    SshConnectResult,
    SftpListing,
    PerfSnapshot
} from "./types"

export function openWorkspace(path: string): Promise<string> {
    return invoke("open_workspace", { path })
}

export function listDir(path: string): Promise<FileNode[]> {
    return invoke("list_dir", { path })
}

export function workspacePathIndex(workspace: string): Promise<WorkspacePathIndexResult> {
    return invoke("workspace_path_index", { workspace })
}

export function openFile(path: string): Promise<OpenFileResult> {
    return invoke("open_file", { path })
}

export function saveFile(path: string, content: string): Promise<number> {
    return invoke("save_file", { path, content })
}

export function fsCreateFile(workspace: string, path: string): Promise<void> {
    return invoke("fs_create_file", { workspace, path })
}

export function fsCreateDir(workspace: string, path: string): Promise<void> {
    return invoke("fs_create_dir", { workspace, path })
}

export function fsRename(workspace: string, from: string, to: string): Promise<void> {
    return invoke("fs_rename", { workspace, from, to })
}

export function fsDelete(workspace: string, path: string): Promise<void> {
    return invoke("fs_delete", { workspace, path })
}

export function startWatch(path: string): Promise<void> {
    return invoke("start_watch", { path })
}

export function gitDetect(path: string): Promise<GitEnvironment> {
    return invoke("git_detect", { path })
}

export function gitStatus(pathspec?: string[]): Promise<GitStatus> {
    return invoke("git_status_cmd", { pathspec: pathspec ?? null })
}

export function gitStage(paths: string[]): Promise<void> {
    return invoke("git_stage", { paths })
}

export function gitUnstage(paths: string[]): Promise<void> {
    return invoke("git_unstage", { paths })
}

export function gitDiscard(paths: string[], untracked: string[]): Promise<void> {
    return invoke("git_discard", { paths, untracked })
}

export function gitCommit(message: string): Promise<void> {
    return invoke("git_commit_cmd", { message })
}

export function gitBranches(): Promise<BranchList> {
    return invoke("git_branches")
}

export function gitCreateBranch(name: string): Promise<void> {
    return invoke("git_create_branch", { name })
}

export function gitCheckout(name: string): Promise<void> {
    return invoke("git_checkout", { name })
}

export function gitCherryPick(hash: string): Promise<void> {
    return invoke("git_cherry_pick", { hash })
}

export function gitFetch(background: boolean): Promise<void> {
    return invoke("git_fetch_cmd", { background })
}

export function gitPull(): Promise<void> {
    return invoke("git_pull_cmd")
}

export function gitPush(): Promise<void> {
    return invoke("git_push_cmd")
}

export function gitRemoteProbe(): Promise<RemoteProbe> {
    return invoke("git_remote_probe")
}

export function gitDiffContent(path: string, staged: boolean): Promise<DiffContent> {
    return invoke("git_diff_content", { path, staged })
}

export function gitConflictAbort(op: string): Promise<void> {
    return invoke("git_conflict_abort", { op })
}

export function gitConflictContinue(op: string): Promise<void> {
    return invoke("git_conflict_continue", { op })
}

export function askpassRespond(id: number, response: string | null): Promise<void> {
    return invoke("askpass_respond", { id, response })
}

export function gitLogPage(
    skip: number,
    limit: number,
    query?: string | null,
    author?: string | null,
    since?: string | null,
    until?: string | null
): Promise<LogPage> {
    return invoke("git_log_page", {
        skip,
        limit,
        query: query ?? null,
        author: author ?? null,
        since: since ?? null,
        until: until ?? null
    })
}

export function gitCommitDetail(hash: string): Promise<CommitDetail> {
    return invoke("git_commit_detail", { hash })
}

export function gitLogAuthors(): Promise<AuthorEntry[]> {
    return invoke("git_log_authors")
}

export function gitFileAtRev(rev: string, path: string): Promise<FileAtRevResult> {
    return invoke("git_file_at_rev", { rev, path })
}

export function searchWorkspace(
    root: string,
    query: string,
    caseSensitive: boolean,
    onEvent: (e: SearchEvent) => void
): Promise<void> {
    const ch = new Channel<SearchEvent>()
    ch.onmessage = onEvent
    return invoke("search_workspace", { root, query, caseSensitive, onEvent: ch })
}

export function ptyOpen(
    workspace: string,
    sessionId: string,
    shell: string | null,
    shellArgs: string[] | undefined,
    cols: number,
    rows: number,
    onEvent: (e: PtyEvent) => void
): Promise<PtySessionInfo> {
    const ch = new Channel<PtyEvent>()
    ch.onmessage = onEvent
    return invoke("pty_open", { workspace, sessionId, shell, shellArgs, cols, rows, onEvent: ch })
}

export function ptyWrite(sessionId: string, data: string): Promise<void> {
    return invoke("pty_write", { sessionId, data })
}

export function ptyResize(sessionId: string, cols: number, rows: number): Promise<void> {
    return invoke("pty_resize", { sessionId, cols, rows })
}

export function ptyClose(sessionId: string): Promise<void> {
    return invoke("pty_close", { sessionId })
}

export function ptyCloseWorkspace(workspace: string): Promise<void> {
    return invoke("pty_close_workspace", { workspace })
}

export function devServerDetect(workspace: string, extraPorts?: number[]): Promise<DevServerDetect> {
    return invoke("dev_server_detect", { workspace, extraPorts })
}

export function devServerStart(
    workspace: string,
    command: string,
    port: number | null,
    onOutput: (line: string) => void
): Promise<DevServerInfo> {
    const ch = new Channel<string>()
    ch.onmessage = onOutput
    return invoke("dev_server_start", { workspace, command, port, onOutput: ch })
}

export function devServerStop(workspace: string): Promise<void> {
    return invoke("dev_server_stop", { workspace })
}

export function devServerStopWorkspace(workspace: string): Promise<void> {
    return invoke("dev_server_stop_workspace", { workspace })
}

export function lspStart(
    workspace: string,
    language: string,
    onMessage: (msg: string) => void
): Promise<LspServerInfo> {
    const ch = new Channel<string>()
    ch.onmessage = onMessage
    return invoke("lsp_start", { workspace, language, onMessage: ch })
}

export function lspSend(workspace: string, language: string, message: string): Promise<void> {
    return invoke("lsp_send", { workspace, language, message })
}

export function lspStopWorkspace(workspace: string): Promise<void> {
    return invoke("lsp_stop_workspace", { workspace })
}

export function lspStatus(workspace: string): Promise<LspServerInfo[]> {
    return invoke("lsp_status", { workspace })
}

export function lspConfigGet(): Promise<LspConfig> {
    return invoke("lsp_config_get")
}

export function lspConfigSetServer(
    workspace: string | null,
    language: string,
    serverId: string
): Promise<LspConfig> {
    return invoke("lsp_config_set_server", { workspace, language, serverId })
}

export function lspConfigStale(): Promise<string[]> {
    return invoke("lsp_config_stale")
}

export function lspConfigClearStale(workspace: string): Promise<LspConfig> {
    return invoke("lsp_config_clear_stale", { workspace })
}

export function lspSetTrace(enabled: boolean): Promise<void> {
    return invoke("lsp_set_trace", { enabled })
}

export function agentSetTrace(enabled: boolean): Promise<void> {
    return invoke("agent_set_trace", { enabled })
}

export function lspInstallServer(
    workspace: string | null,
    language: string
): Promise<LspServerInfo> {
    return invoke("lsp_install_server", { workspace, language })
}

export function agentList(cwd: string): Promise<string[]> {
    return invoke("agent_list", { cwd })
}

export function dbOpen(config: DbOpenConfig): Promise<{ connId: string }> {
    return invoke("db_open", { config })
}

export function dbClose(connId: string): Promise<void> {
    return invoke("db_close", { connId })
}

export function dbListTables(connId: string): Promise<DbTable[]> {
    return invoke("db_list_tables", { connId })
}

export function dbTableColumns(connId: string, table: string): Promise<DbColumn[]> {
    return invoke("db_table_columns", { connId, table })
}

export function dbQuery(connId: string, sql: string, maxRows?: number): Promise<DbQueryResult> {
    return invoke("db_query", { connId, sql, maxRows: maxRows ?? null })
}

export function agentKill(id: string, reason?: string): Promise<void> {
    return invoke("agent_kill", { id, reason: reason ?? null })
}

export function agentStderrTail(id: string): Promise<string[]> {
    return invoke("agent_stderr_tail", { id })
}

export function sshConnect(
    host: string,
    port: number,
    user: string,
    auth: SshAuthInput
): Promise<SshConnectResult> {
    return invoke("ssh_connect", { host, port, user, auth })
}

export function sshOpenShell(sessionId: string, cols: number, rows: number): Promise<void> {
    return invoke("ssh_open_shell", { sessionId, cols, rows })
}

export function sshWrite(sessionId: string, data: string): Promise<void> {
    return invoke("ssh_write", { sessionId, data })
}

export function sshResize(sessionId: string, cols: number, rows: number): Promise<void> {
    return invoke("ssh_resize", { sessionId, cols, rows })
}

export function sshDisconnect(sessionId: string): Promise<void> {
    return invoke("ssh_disconnect", { sessionId })
}

// --- SFTP (F5): browse + chunked transfers over the live SSH session ---
export function sftpListDir(sessionId: string, path: string): Promise<SftpListing> {
    return invoke("sftp_list_dir", { sessionId, path })
}

export function sftpMkdir(sessionId: string, path: string): Promise<void> {
    return invoke("sftp_mkdir", { sessionId, path })
}

export function sftpRename(sessionId: string, from: string, to: string): Promise<void> {
    return invoke("sftp_rename", { sessionId, from, to })
}

export function sftpRemove(sessionId: string, path: string, isDir: boolean): Promise<void> {
    return invoke("sftp_remove", { sessionId, path, isDir })
}

export function sftpUpload(
    sessionId: string,
    transferId: string,
    localPath: string,
    remoteDir: string
): Promise<void> {
    return invoke("sftp_upload", { sessionId, transferId, localPath, remoteDir })
}

export function sftpDownload(
    sessionId: string,
    transferId: string,
    remotePath: string,
    localPath: string
): Promise<void> {
    return invoke("sftp_download", { sessionId, transferId, remotePath, localPath })
}

export function perfSnapshot(): Promise<PerfSnapshot | null> {
    return invoke("perf_snapshot")
}

// --- Preview (P3): local static server + external-URL child webview ---
export function previewServe(dir: string): Promise<number> {
    return invoke("preview_serve", { dir })
}

export function previewStopAll(): Promise<void> {
    return invoke("preview_stop_all")
}

export function previewOpenUrl(
    url: string,
    x: number,
    y: number,
    width: number,
    height: number
): Promise<void> {
    return invoke("preview_open_url", { url, x, y, width, height })
}

export function previewSetBounds(
    x: number,
    y: number,
    width: number,
    height: number
): Promise<void> {
    return invoke("preview_set_bounds", { x, y, width, height })
}

export function previewSetVisible(visible: boolean): Promise<void> {
    return invoke("preview_set_visible", { visible })
}

export function previewClose(): Promise<void> {
    return invoke("preview_close")
}

export function previewBack(): Promise<void> {
    return invoke("preview_back")
}

export function previewForward(): Promise<void> {
    return invoke("preview_forward")
}

export function previewReload(): Promise<void> {
    return invoke("preview_reload")
}
