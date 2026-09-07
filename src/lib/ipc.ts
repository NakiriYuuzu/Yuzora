import { Channel, invoke } from "@tauri-apps/api/core"
import { parseRemoteFilePath } from "./runtimeIdentity"
import { invokeNativeGit, closeNativeGitWorkspace } from "./nativeGit"

// Re-exported so feature modules that carry their own domain logic around a
// command (the log-event envelope builder) can
// reach the IPC boundary through this module instead of importing the Tauri
// core directly. `@tauri-apps/api/core` should be imported only here and in
// `platform.ts` (which owns `isTauri`).
export { invoke }

export function sshSessionAlive(sessionId: string): Promise<boolean> {
    return invoke("ssh_session_alive", { sessionId })
}

import type {
    FileNode,
    WorkspaceOpenResult,
    WorkspacePathIndexResult,
    OpenFileResult,
    GitEnvironment,
    GitBootstrapResult,
    WorkspaceTrustStatus,
    WorkspaceTrustChallenge,
    WorkspaceExecutionChallenge,
    TrustedWorkspace,
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
    LspInstallProgress,
    PtyActivity,
    PtyEvent,
    PtyOutputMetrics,
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
    DbPostgresTransportChallenge,
    DbPostgresTransportChallengeRequest,
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
    SftpDownloadDest,
    SftpListing,
    SftpUploadSource,
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


export function openWorkspace(path: string): Promise<WorkspaceOpenResult> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.openRemoteWorkspace(path))
    return invoke("open_workspace", { path })
}

export function listDir(path: string): Promise<FileNode[]> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.listRemoteDir(path))
    return invoke("list_dir", { path })
}

export function workspacePathIndex(workspace: string): Promise<WorkspacePathIndexResult> {
    if (parseRemoteFilePath(workspace)) return import("./remoteFiles").then((remote) => remote.indexRemoteWorkspace(workspace))
    return invoke("workspace_path_index", { workspace })
}

export function openFile(path: string): Promise<OpenFileResult> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.readRemoteFile(path))
    return invoke("open_file", { path })
}

/** Accept the revision only when the editor accepts the matching content. */
export async function openFileSnapshot(path: string): Promise<{ result: OpenFileResult; accept: () => void }> {
    if (parseRemoteFilePath(path)) return (await import("./remoteFiles")).readRemoteFileSnapshot(path)
    return { result: await openFile(path), accept: () => {} }
}

export function isOpenableFile(path: string): Promise<boolean> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.readRemoteFile(path, false)).then(() => true, () => false)
    return invoke("is_openable_file", { path })
}

export function allowWorkspaceAssetScope(path: string): Promise<void> {
    if (parseRemoteFilePath(path)) return Promise.resolve()
    return invoke("allow_workspace_asset_scope", { path })
}

export interface FileBase64 {
    data: string
    size: number
}

export function readFileBase64(path: string, maxBytes: number): Promise<FileBase64> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.readRemoteBase64(path, maxBytes))
    return invoke("read_file_base64", { path, maxBytes })
}

export function saveFile(path: string, content: string): Promise<number> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.saveRemoteFile(path, content))
    return invoke("save_file", { path, content })
}

export function fsCreateFile(workspace: string, path: string): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.createRemotePath(workspace, path, false))
    return invoke("fs_create_file", { workspace, path })
}

export function fsCreateDir(workspace: string, path: string): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.createRemotePath(workspace, path, true))
    return invoke("fs_create_dir", { workspace, path })
}

export function fsRename(workspace: string, from: string, to: string): Promise<void> {
    if ([workspace, from, to].some((path) => parseRemoteFilePath(path))) return import("./remoteFiles").then((remote) => remote.renameRemotePath(workspace, from, to))
    return invoke("fs_rename", { workspace, from, to })
}

export function fsDelete(workspace: string, path: string): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.deleteRemotePath(workspace, path))
    return invoke("fs_delete", { workspace, path })
}

let watchRequestGeneration = 0
export async function startWatch(path: string): Promise<void> {
    const generation = ++watchRequestGeneration
    const remote = await import("./remoteFiles")
    if (generation !== watchRequestGeneration) return
    await remote.stopRemoteWatch()
    if (generation !== watchRequestGeneration) return
    if (parseRemoteFilePath(path)) {
        await invoke("stop_watch")
        if (generation !== watchRequestGeneration) return
        return remote.startRemoteWatch(path)
    }
    return invoke("start_watch", { path })
}

function invokeGit<T>(command: string, args: Record<string, unknown>): Promise<T> {
    const path = args.repositoryRoot ?? args.path
    if (typeof path === "string" && parseRemoteFilePath(path)) return import("./remoteGit").then((remote) => remote.invokeRemoteGit<T>(command, args))
    return invokeNativeGit<T>(command, args)
}

export function gitCloseWorkspace(path: string): void {
    if (!parseRemoteFilePath(path)) closeNativeGitWorkspace(path)
}

export function gitDetect(path: string): Promise<GitEnvironment> {
    return invokeGit("git_detect", { path })
}

// #57 T3：冷開 workspace 的 git 首載——一趟完成 detect→(status‖branches)，
// 消除 detect 先行寫 State、status/branches 才能發的結構性 waterfall。
// 細粒度 gitStatus/gitBranches 保留給後續 refresh。
export function gitBootstrap(path: string): Promise<GitBootstrapResult> {
    return invokeGit("git_bootstrap", { path })
}

export function workspaceTrustStatus(path: string): Promise<WorkspaceTrustStatus> {
    if (parseRemoteFilePath(path)) return import("./remoteTrust").then((remote) => remote.remoteTrustStatus(path))
    return invoke("workspace_trust_status", { path })
}

export async function workspaceTrustList(): Promise<TrustedWorkspace[]> {
    const [local, remote] = await Promise.all([invoke<TrustedWorkspace[]>("workspace_trust_list"), import("./remoteTrust").then((remote) => remote.remoteTrustList())])
    return [...local, ...remote]
}

export function workspaceTrustChallenge(path: string): Promise<WorkspaceTrustChallenge> {
    if (parseRemoteFilePath(path)) return import("./remoteTrust").then((remote) => remote.remoteTrustChallenge(path))
    return invoke("workspace_trust_challenge", { path })
}

export function workspaceTrustExecutionChallenge(
    path: string,
    command: string
): Promise<WorkspaceExecutionChallenge> {
    if (parseRemoteFilePath(path)) return import("./remoteTrust").then((remote) => remote.remoteExecutionChallenge(path, command))
    return invoke("workspace_trust_execution_challenge", { path, command })
}

export function workspaceTrustGrant(challengeId: string): Promise<WorkspaceTrustStatus> {
    if (challengeId.startsWith("[")) return import("./remoteTrust").then((remote) => remote.remoteTrustGrant(challengeId))
    return invoke("workspace_trust_grant", { challengeId })
}

export function workspaceTrustRevoke(canonicalPath: string): Promise<TrustedWorkspace[]> {
    if (parseRemoteFilePath(canonicalPath)) return import("./remoteTrust").then(async (remote) => { await remote.remoteTrustRevoke(canonicalPath); return workspaceTrustList() })
    return invoke("workspace_trust_revoke", { canonicalPath })
}

export function gitStatus(repositoryRoot: string, pathspec?: string[]): Promise<GitStatus> {
    return invokeGit("git_status_cmd", { repositoryRoot, pathspec: pathspec ?? null })
}

export function gitStage(repositoryRoot: string, paths: string[]): Promise<void> {
    return invokeGit("git_stage", { repositoryRoot, paths })
}

export function gitUnstage(repositoryRoot: string, paths: string[]): Promise<void> {
    return invokeGit("git_unstage", { repositoryRoot, paths })
}

export function gitDiscard(
    repositoryRoot: string,
    paths: string[],
    untracked: string[]
): Promise<void> {
    return invokeGit("git_discard", { repositoryRoot, paths, untracked })
}

export function gitRollbackPaths(
    repositoryRoot: string,
    targets: GitRollbackTarget[],
    deleteUntrackedOrAdded: boolean
): Promise<GitRollbackResult> {
    return invokeGit("git_rollback_paths", { repositoryRoot, targets, deleteUntrackedOrAdded })
}

export function gitCommit(repositoryRoot: string, message: string): Promise<void> {
    return invokeGit("git_commit_cmd", { repositoryRoot, message })
}

export function gitBranches(repositoryRoot: string): Promise<BranchList> {
    return invokeGit("git_branches", { repositoryRoot })
}

export function gitCreateBranch(
    repositoryRoot: string,
    name: string,
    startPoint?: string
): Promise<void> {
    return invokeGit("git_create_branch", { repositoryRoot, name, startPoint: startPoint ?? null })
}

export function gitCheckout(repositoryRoot: string, name: string): Promise<void> {
    return invokeGit("git_checkout", { repositoryRoot, name })
}

export function gitCheckoutDetached(repositoryRoot: string, rev: string): Promise<void> {
    return invokeGit("git_checkout_detached", { repositoryRoot, rev })
}

export function gitCherryPick(repositoryRoot: string, hash: string): Promise<void> {
    return invokeGit("git_cherry_pick", { repositoryRoot, hash })
}

export function gitFetch(repositoryRoot: string, background: boolean): Promise<void> {
    return invokeGit("git_fetch_cmd", { repositoryRoot, background })
}

export function gitPull(repositoryRoot: string): Promise<void> {
    return invokeGit("git_pull_cmd", { repositoryRoot })
}

export function gitPush(repositoryRoot: string): Promise<void> {
    return invokeGit("git_push_cmd", { repositoryRoot })
}

export function gitRemoteProbe(repositoryRoot: string): Promise<RemoteProbe> {
    return invokeGit("git_remote_probe", { repositoryRoot })
}

export function gitDiffContent(
    repositoryRoot: string,
    path: string,
    staged: boolean,
    origPath?: string | null
): Promise<DiffContent> {
    return invokeGit("git_diff_content", { repositoryRoot, path, staged, origPath: origPath ?? null })
}

export function gitConflictAbort(repositoryRoot: string, op: string): Promise<void> {
    return invokeGit("git_conflict_abort", { repositoryRoot, op })
}

export function gitConflictContinue(repositoryRoot: string, op: string): Promise<void> {
    return invokeGit("git_conflict_continue", { repositoryRoot, op })
}

export function askpassRespond(id: number, response: string | null): Promise<void> {
    return invoke("askpass_respond", { id, response })
}

export function gitLogPage(
    repositoryRoot: string,
    cursor: string | null,
    limit: number,
    query?: string | null,
    author?: string | null,
    since?: string | null,
    until?: string | null
): Promise<LogPage> {
    return invokeGit("git_log_page", {
        repositoryRoot,
        cursor,
        limit,
        query: query ?? null,
        author: author ?? null,
        since: since ?? null,
        until: until ?? null
    })
}

export function gitCommitDetail(repositoryRoot: string, hash: string): Promise<CommitDetail> {
    return invokeGit("git_commit_detail", { repositoryRoot, hash })
}

export function gitLogAuthors(repositoryRoot: string): Promise<AuthorEntry[]> {
    return invokeGit("git_log_authors", { repositoryRoot })
}

export function gitFileAtRev(
    repositoryRoot: string,
    rev: string,
    path: string
): Promise<FileAtRevResult> {
    return invokeGit("git_file_at_rev", { repositoryRoot, rev, path })
}

export function searchWorkspace(
    root: string,
    query: string,
    caseSensitive: boolean,
    onEvent: (e: SearchEvent) => void
): Promise<void> {
    if (parseRemoteFilePath(root)) return import("./remoteSearch").then((remote) => remote.searchRemoteWorkspace(root, query, caseSensitive, onEvent))
    const ch = new Channel<SearchEvent>()
    ch.onmessage = onEvent
    return import("./remoteSearch").then(async (remote) => {
        await remote.stopRemoteSearch()
        return invoke("search_workspace", { root, query, caseSensitive, onEvent: ch })
    })
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

export function ptyOutputMetrics(sessionId: string): Promise<PtyOutputMetrics | null> {
    return invoke("pty_output_metrics", { sessionId })
}

export function ptyClose(sessionId: string): Promise<void> {
    return invoke("pty_close", { sessionId })
}

export function ptyCloseWorkspace(workspace: string): Promise<void> {
    return invoke("pty_close_workspace", { workspace })
}

export function devServerDetect(workspace: string, extraPorts?: number[]): Promise<DevServerDetect> {
    if (parseRemoteFilePath(workspace)) return import("./remoteDevServer").then((remote) => remote.detectRemoteDevServer(workspace, extraPorts))
    return invoke("dev_server_detect", { workspace, extraPorts })
}

export function devServerStart(
    workspace: string,
    command: string,
    port: number | null,
    onOutput: (line: string) => void,
    challengeId: string
): Promise<DevServerInfo> {
    if (parseRemoteFilePath(workspace)) return import("./remoteDevServer").then((remote) => remote.startRemoteDevServer(workspace, command, port, onOutput, challengeId))
    const ch = new Channel<string>()
    ch.onmessage = onOutput
    return invoke("dev_server_start", { workspace, command, port, challengeId, onOutput: ch })
}

export function devServerStop(workspace: string): Promise<void> {
    if (parseRemoteFilePath(workspace)) return import("./remoteDevServer").then((remote) => remote.stopRemoteDevServer(workspace))
    return invoke("dev_server_stop", { workspace })
}

export function devServerStopWorkspace(workspace: string): Promise<void> {
    if (parseRemoteFilePath(workspace)) return import("./remoteDevServer").then((remote) => remote.stopRemoteDevServer(workspace))
    return invoke("dev_server_stop_workspace", { workspace })
}

export function lspStart(
    workspace: string,
    language: string,
    onMessage: (msg: string) => void
): Promise<LspServerInfo> {
    if (parseRemoteFilePath(workspace)) return import("./remoteLsp").then((remote) => remote.startRemoteLsp(workspace, language, onMessage))
    const ch = new Channel<string>()
    ch.onmessage = onMessage
    return invoke("lsp_start", { workspace, language, onMessage: ch })
}

export function lspSend(workspace: string, language: string, message: string): Promise<void> {
    if (parseRemoteFilePath(workspace)) return import("./remoteLsp").then((remote) => remote.sendRemoteLsp(workspace, language, message))
    return invoke("lsp_send", { workspace, language, message })
}

export function lspStopWorkspace(workspace: string): Promise<void> {
    if (parseRemoteFilePath(workspace)) return import("./remoteLsp").then((remote) => remote.stopRemoteLsp(workspace))
    return invoke("lsp_stop_workspace", { workspace })
}

export function lspStatus(workspace: string): Promise<LspServerInfo[]> {
    if (parseRemoteFilePath(workspace)) return import("./remoteLsp").then((remote) => remote.remoteLspStatus(workspace))
    return invoke("lsp_status", { workspace })
}

export function lspDetectServer(
    workspace: string | null,
    language: string,
    hostWorkspace?: string
): Promise<LspServerInfo> {
    if (hostWorkspace) return import("./remoteLspConfig").then((remote) => remote.remoteLspConfigDetect(hostWorkspace, language, workspace === null))
    if (workspace && parseRemoteFilePath(workspace)) return import("./remoteLsp").then((remote) => remote.detectRemoteLsp(workspace, language))
    return invoke("lsp_detect_server", { workspace, language })
}

export function lspConfigGet(hostWorkspace?: string): Promise<LspConfig> {
    if (hostWorkspace) return import("./remoteLspConfig").then((remote) => remote.remoteLspConfigGet(hostWorkspace))
    return invoke("lsp_config_get")
}

export function lspConfigSetServer(
    workspace: string | null,
    language: string,
    serverId: string,
    hostWorkspace?: string
): Promise<LspConfig> {
    const remoteWorkspace = hostWorkspace ?? (workspace && parseRemoteFilePath(workspace) ? workspace : undefined)
    if (remoteWorkspace) return import("./remoteLspConfig").then((remote) => remote.remoteLspConfigSet(remoteWorkspace, workspace, language, serverId))
    return invoke("lsp_config_set_server", { workspace, language, serverId })
}

export function lspConfigStale(hostWorkspace?: string): Promise<string[]> {
    if (hostWorkspace) return import("./remoteLspConfig").then((remote) => remote.remoteLspConfigStale(hostWorkspace))
    return invoke("lsp_config_stale")
}

export function lspConfigClearStale(workspace: string, hostWorkspace?: string): Promise<LspConfig> {
    if (hostWorkspace || parseRemoteFilePath(workspace)) return import("./remoteLspConfig").then((remote) => remote.remoteLspConfigClear(hostWorkspace ?? workspace, workspace))
    return invoke("lsp_config_clear_stale", { workspace })
}

export function lspSetTrace(enabled: boolean, hostWorkspace?: string): Promise<void> {
    if (hostWorkspace) return import("./remoteLsp").then((remote) => remote.setRemoteLspTrace(hostWorkspace, enabled))
    return invoke("lsp_set_trace", { enabled })
}


export function lspInstallServer(
    workspace: string | null,
    language: string,
    hostWorkspace?: string,
    onProgress?: (event: LspInstallProgress) => void
): Promise<LspServerInfo> {
    const remoteWorkspace = hostWorkspace ?? (workspace && parseRemoteFilePath(workspace) ? workspace : undefined)
    if (remoteWorkspace) return import("./remoteLspInstall").then((remote) => remote.installRemoteLsp(remoteWorkspace, workspace, language, onProgress))
    return invoke("lsp_install_server", { workspace, language })
}

export function lspCancelInstall(context: string, language: string): void {
    void import("./remoteLspInstall").then((remote) => remote.cancelRemoteLspInstall(context, language))
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

export function dbPostgresTransportChallenge(
    request: DbPostgresTransportChallengeRequest
): Promise<DbPostgresTransportChallenge> {
    return invoke("db_postgres_transport_challenge", { request })
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



export function sshConnect(
    host: string,
    port: number,
    user: string,
    auth: SshAuthInput
): Promise<SshConnectResult> {
    return invoke("ssh_connect", { host, port, user, auth })
}

export function sshHostKeyRespond(
    challengeId: string,
    accept: boolean,
    endpoint: string,
    fingerprint: string
): Promise<void> {
    return invoke("ssh_host_key_respond", { challengeId, accept, endpoint, fingerprint })
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

export interface SftpSelectedPathGrant {
    id: string
    leaf: string
}

export function sftpPickSelectedPath(): Promise<SftpSelectedPathGrant[]> {
    return invoke("sftp_pick_selected_path")
}

export function sftpPickTree(direction: "upload" | "download", suggestedLeaf?: string): Promise<SftpSelectedPathGrant | null> {
    return invoke("sftp_pick_tree", { direction, suggestedLeaf })
}

export function sftpTransferTree(sessionId: string, request: { selectionId: string; transferId: string; direction: "upload" | "download"; remotePath: string; name: string }): Promise<{ files: number; bytes: number }> {
    return invoke("sftp_transfer_tree", { sessionId, request })
}

export function sftpPickDownloadDestination(
    suggestedLeaf: string
): Promise<SftpSelectedPathGrant | null> {
    return invoke("sftp_pick_download_destination", { suggestedLeaf })
}

export function sftpUpload(
    sessionId: string,
    transferId: string,
    source: SftpUploadSource,
    remoteDir: string,
    expectedRevision: string | null = null
): Promise<void> {
    return invoke("sftp_upload", { sessionId, request: { transferId, source, remoteDir, expectedRevision } })
}

export function sftpFileRevision(sessionId: string, path: string, transferId: string): Promise<string | null> {
    return invoke("sftp_file_revision", { sessionId, path, transferId })
}

export function sftpTransferPrepare(sessionId: string): Promise<string> {
    return invoke("sftp_transfer_prepare", { sessionId })
}

export function sftpTransferCancel(sessionId: string, transferId: string): Promise<void> {
    return invoke("sftp_transfer_cancel", { sessionId, transferId })
}

export function sftpDownload(
    sessionId: string,
    transferId: string,
    remotePath: string,
    dest: SftpDownloadDest
): Promise<void> {
    return invoke("sftp_download", {
        sessionId,
        transferId,
        remotePath,
        destinationCapabilityId: dest.capabilityId
    })
}

export function perfSnapshot(): Promise<PerfSnapshot | null> {
    return invoke("perf_snapshot")
}

// --- Preview (P3): isolated static session + external-URL child webview ---
export type PreviewSession = {
    token: string
    url: string
    sourcePath?: string
}

export function previewCreate(path: string): Promise<PreviewSession> {
    if (parseRemoteFilePath(path)) return import("./remotePreview").then((remote) => remote.createRemotePreview(path))
    return invoke("preview_create", { path })
}

export function previewRevoke(token: string): Promise<void> {
    if (token.startsWith("host-preview:")) return import("./remotePreview").then((remote) => remote.revokeRemotePreview(token))
    return invoke("preview_revoke", { token })
}

export async function previewStopAll(): Promise<void> {
    await (await import("./remotePreview")).stopRemotePreviews()
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
