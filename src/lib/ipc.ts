import { Channel, invoke } from "@tauri-apps/api/core"

// Re-exported so feature modules that carry their own domain logic around a
// command (agent/ACP protocol handlers, the log-event envelope builder) can
// reach the IPC boundary through this module instead of importing the Tauri
// core directly. `@tauri-apps/api/core` should be imported only here and in
// `platform.ts` (which owns `isTauri`).
export { invoke }

import type { AgentId, AgentRuntimeAvailability } from "./agentPresets"
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
    PtyActivity,
    PtyEvent,
    PtySessionInfo,
    TerminalProfile,
    TerminalCwdStrategy,
    DevServerDetect,
    DevServerInfo,
    DbTable,
    DbColumn,
    DbDescriptorId,
    DbProfileDescriptor,
    DbProfileLoadResult,
    DbLegacyProfileImportRequest,
    DbProfileCreateRequest,
    DbProfileUpdateRequest,
    DbProfileRecoveryRequest,
    DbSaveAndConnectOutcome,
    DbTestConnectionRequest,
    DbTestConnectionResult,
    DbLiveConnection,
    DbConnectionIdentity,
    DbQueryRunOwner,
    DbQueryCancelResult,
    DbQueryRunRequest,
    DbQueryRun,
    DbResultSessionOwner,
    DbResultPageRequest,
    DbResultPage,
    SshAuthInput,
    SshConnectResult,
    SftpListing,
    PerfSnapshot
} from "./types"

/** Full latest-status snapshot for one deduplicated rollback path. */
export type GitRollbackClassification =
    | {
          kind: "tracked"
          stagedStatus: string | null
          unstagedStatus: string | null
          origPath: string | null
      }
    | {
          kind: "added"
          stagedStatus: string | null
          unstagedStatus: string | null
      }
    | { kind: "untracked" }
    | { kind: "conflicted" }

export interface GitRollbackTarget {
    path: string
    classification: GitRollbackClassification
}

export interface GitRollbackResult {
    restored: string[]
    preservedUntracked: string[]
    deleted: string[]
}

export interface AgentLatestVersion {
    agentId: AgentId
    version: string
}

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

export function allowWorkspaceAssetScope(path: string): Promise<void> {
    return invoke("allow_workspace_asset_scope", { path })
}

export interface FileBase64 {
    data: string
    size: number
}

export function readFileBase64(path: string, maxBytes: number): Promise<FileBase64> {
    return invoke("read_file_base64", { path, maxBytes })
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

export function gitStage(repositoryRoot: string, paths: string[]): Promise<void> {
    return invoke("git_stage", { repositoryRoot, paths })
}

export function gitUnstage(repositoryRoot: string, paths: string[]): Promise<void> {
    return invoke("git_unstage", { repositoryRoot, paths })
}

export function gitDiscard(
    repositoryRoot: string,
    paths: string[],
    untracked: string[]
): Promise<void> {
    return invoke("git_discard", { repositoryRoot, paths, untracked })
}

export function gitRollbackPaths(
    repositoryRoot: string,
    targets: GitRollbackTarget[],
    deleteUntrackedOrAdded: boolean
): Promise<GitRollbackResult> {
    return invoke("git_rollback_paths", { repositoryRoot, targets, deleteUntrackedOrAdded })
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

export function gitFetch(background: boolean, repositoryRoot?: string): Promise<void> {
    return invoke("git_fetch_cmd", {
        background,
        ...(repositoryRoot ? { repositoryRoot } : {})
    })
}

export function gitPull(repositoryRoot?: string): Promise<void> {
    return invoke("git_pull_cmd", repositoryRoot ? { repositoryRoot } : undefined)
}

export function gitPush(repositoryRoot?: string): Promise<void> {
    return invoke("git_push_cmd", repositoryRoot ? { repositoryRoot } : undefined)
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
    cwdStrategy: TerminalCwdStrategy,
    cols: number,
    rows: number,
    onEvent: (e: PtyEvent) => void
): Promise<PtySessionInfo> {
    const ch = new Channel<PtyEvent>()
    ch.onmessage = onEvent
    return invoke("pty_open", {
        workspace,
        sessionId,
        shell,
        shellArgs,
        cwdStrategy,
        cols,
        rows,
        onEvent: ch
    })
}

export function ptyListProfiles(): Promise<TerminalProfile[]> {
    return invoke("pty_list_profiles")
}

export function ptyWrite(sessionId: string, data: string): Promise<void> {
    return invoke("pty_write", { sessionId, data })
}

export function ptyResize(sessionId: string, cols: number, rows: number): Promise<void> {
    return invoke("pty_resize", { sessionId, cols, rows })
}

export function ptyActivity(sessionId: string): Promise<PtyActivity> {
    return invoke("pty_activity", { sessionId })
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

export function lspDetectServer(
    workspace: string | null,
    language: string
): Promise<LspServerInfo> {
    return invoke("lsp_detect_server", { workspace, language })
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

export function agentDetectRuntimes(): Promise<AgentRuntimeAvailability> {
    return invoke("agent_detect_runtimes")
}

export function agentLatestVersions(): Promise<AgentLatestVersion[]> {
    return invoke("agent_latest_versions")
}

export function dbListTables(identity: DbConnectionIdentity): Promise<DbTable[]> {
    return invoke("db_list_tables", { identity })
}

export function dbTableColumns(
    identity: DbConnectionIdentity,
    table: DbTable
): Promise<DbColumn[]> {
    return invoke("db_table_columns", { identity, table })
}

// --- Database v2 contract seams (P1) ---
// These commands are intentionally thin invokes. Later phases own their Rust
// implementations; an unavailable command must reject instead of being
// replaced with optimistic frontend state or a synthetic success response.
export function dbProfileList(): Promise<DbProfileLoadResult> {
    return invoke("db_profile_list")
}

export function dbProfileImportLegacy(
    request: DbLegacyProfileImportRequest
): Promise<DbProfileLoadResult> {
    return invoke("db_profile_import_legacy", { request })
}

export function dbProfileCreate(request: DbProfileCreateRequest): Promise<DbSaveAndConnectOutcome> {
    return invoke("db_profile_create", { request })
}

export function dbProfileUpdate(request: DbProfileUpdateRequest): Promise<DbProfileDescriptor> {
    return invoke("db_profile_update", { request })
}

export function dbProfileRemoveCredential(
    descriptorId: DbDescriptorId
): Promise<DbProfileLoadResult> {
    return invoke("db_profile_remove_credential", { descriptorId })
}

export function dbProfileForget(descriptorId: DbDescriptorId): Promise<DbProfileLoadResult> {
    return invoke("db_profile_forget", { descriptorId })
}

export function dbProfileRecover(request: DbProfileRecoveryRequest): Promise<DbProfileLoadResult> {
    return invoke("db_profile_recover", { request })
}

export function dbProfileOpen(descriptorId: DbDescriptorId): Promise<DbLiveConnection> {
    return invoke("db_profile_open", { descriptorId })
}

export function dbProfileDisconnect(identity: DbConnectionIdentity): Promise<void> {
    return invoke("db_profile_disconnect", { identity })
}

export function dbTestConnection(request: DbTestConnectionRequest): Promise<DbTestConnectionResult> {
    return invoke("db_test_connection", { request })
}

export function dbQueryRun(request: DbQueryRunRequest): Promise<DbQueryRun> {
    return invoke("db_query_run", { request })
}

export function dbQueryCancel(owner: DbQueryRunOwner): Promise<DbQueryCancelResult> {
    return invoke("db_query_cancel", { owner })
}

export function dbResultPage(request: DbResultPageRequest): Promise<DbResultPage> {
    return invoke("db_result_page", { request })
}

export function dbResultPagePrevious(owner: DbResultSessionOwner): Promise<DbResultPage> {
    return dbResultPage({ owner, direction: "previous" })
}

export function dbResultPageNext(owner: DbResultSessionOwner): Promise<DbResultPage> {
    return dbResultPage({ owner, direction: "next" })
}

export function dbResultSessionRelease(owner: DbResultSessionOwner): Promise<DbResultPage> {
    return invoke("db_result_session_release", { owner })
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
