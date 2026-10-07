import { Channel, invoke } from "@tauri-apps/api/core"
import { parseRemoteFilePath } from "./runtimeIdentity"
import { invokeNativeGit, closeNativeGitWorkspace } from "./nativeGit"
import { notifyFileSaved } from "./fileSaveEvents"
import type { PreviewInteractionSnapshot, PreviewResourceLease, PreviewResourceSource, PreviewShortcutBinding } from "./previewTypes"

// Re-exported so feature modules that carry their own domain logic around a
// command (the log-event envelope builder) can
// reach the IPC boundary through this module instead of importing the Tauri
// core directly. `@tauri-apps/api/core` should be imported only here and in
// `platform.ts` (which owns `isTauri`).
export { invoke }

/** Top-level activation, including Windows when a child WebView has lost focus. */
export const isWorkbenchWindowActive = () => invoke<boolean>("workbench_is_window_active")

import type {
    DeleteProgress,
    FileNode,
    WorkspaceOpenResult,
    OpenFileResult,
    GitBootstrapResult,
    WorkspaceTrustStatus,
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
    PerfSnapshot, GitDiscovery, GitConflictSides, GitOperationOutcome, GitStashEntry, GitResetMode
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

export function openWorkspaceDirectory(workspaceId: string, path: string): Promise<void> {
    return invoke("open_workspace_directory", { workspaceId, path })
}

export function searchWorkspaceFileNames(root: string, query: string): Promise<import("./fileNameSearchTypes").FileNameSearchResponse> {
    return invoke("search_workspace_file_names", { root, query })
}

export function listDir(path: string): Promise<FileNode[]> {
    if (parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.listRemoteDir(path))
    return invoke("list_dir", { path })
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
    const saved = parseRemoteFilePath(path)
        ? import("./remoteFiles").then((remote) => remote.saveRemoteFile(path, content))
        : invoke<number>("save_file", { path, content })
    return saved.then((result) => { notifyFileSaved(path); return result })
}

async function localMutation(workspace: string, paths: string[]) {
    const { useWorkspaceStore } = await import("@/state/workspaceStore")
    const { relativePathWithin } = await import("./paths")
    const state = useWorkspaceStore.getState()
    if (state.workspacePath !== workspace || !state.workspaceCapabilityId) throw new Error("workspace-capability-missing")
    const relative = paths.map(path => {
        const value = relativePathWithin(workspace, path)
        if (!value || value.includes("\0") || value.split("/").some(part => part === "." || part === ".." || !part)) throw new Error("path-outside-workspace")
        return value
    })
    return { workspaceCapabilityId: state.workspaceCapabilityId, relative }
}

export function fsCreateFile(workspace: string, path: string): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.createRemotePath(workspace, path, false))
    return localMutation(workspace, [path]).then(({ workspaceCapabilityId, relative }) => invoke("fs_create_file", { workspaceCapabilityId, path: relative[0] }))
}

export function fsCreateDir(workspace: string, path: string): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.createRemotePath(workspace, path, true))
    return localMutation(workspace, [path]).then(({ workspaceCapabilityId, relative }) => invoke("fs_create_dir", { workspaceCapabilityId, path: relative[0] }))
}

export function fsRename(workspace: string, from: string, to: string): Promise<void> {
    if ([workspace, from, to].some((path) => parseRemoteFilePath(path))) return import("./remoteFiles").then((remote) => remote.renameRemotePath(workspace, from, to))
    return localMutation(workspace, [from, to]).then(({ workspaceCapabilityId, relative }) => invoke("fs_rename", { workspaceCapabilityId, from: relative[0], to: relative[1] }))
}

async function localTargetDir(workspace: string, targetDir: string): Promise<string> {
    if (targetDir === workspace) return ""
    const [relative] = (await localMutation(workspace, [targetDir])).relative
    return relative
}

async function absoluteInWorkspace(workspace: string, relative: string[]): Promise<string[]> {
    const { isWindowsPath, nativePathJoin } = await import("./paths")
    return relative.map((path) => nativePathJoin(workspace, isWindowsPath(workspace) ? path.replace(/\//g, "\\") : path))
}

/** Recursively copies files/folders into `targetDir` (never overwrites; collisions get a "copy" name). Returns the created paths. */
export async function fsCopyPaths(workspace: string, sources: string[], targetDir: string): Promise<string[]> {
    if ([workspace, targetDir, ...sources].some((path) => parseRemoteFilePath(path))) {
        return import("./remoteFiles").then((remote) => remote.copyRemotePaths(workspace, sources, targetDir))
    }
    const { workspaceCapabilityId, relative } = await localMutation(workspace, sources)
    const created = await invoke<string[]>("fs_copy_paths", { workspaceCapabilityId, sources: relative, targetDir: await localTargetDir(workspace, targetDir) })
    return absoluteInWorkspace(workspace, created)
}

/** Moves files/folders into `targetDir` inside the same workspace. Returns their new paths. */
export async function fsMovePaths(workspace: string, sources: string[], targetDir: string): Promise<string[]> {
    if ([workspace, targetDir, ...sources].some((path) => parseRemoteFilePath(path))) {
        return import("./remoteFiles").then((remote) => remote.moveRemotePaths(workspace, sources, targetDir))
    }
    const { workspaceCapabilityId, relative } = await localMutation(workspace, sources)
    const moved = await invoke<string[]>("fs_move_paths", { workspaceCapabilityId, sources: relative, targetDir: await localTargetDir(workspace, targetDir) })
    return absoluteInWorkspace(workspace, moved)
}

/** Copies the files currently on the OS clipboard (e.g. from Finder or Explorer) into `targetDir`. Local workspaces only. */
export async function fsPasteClipboardFiles(workspace: string, targetDir: string): Promise<string[]> {
    if (parseRemoteFilePath(workspace)) throw new Error("clipboard-import-remote-unsupported")
    const { workspaceCapabilityId } = await localMutation(workspace, [])
    const created = await invoke<string[]>("fs_paste_clipboard_files", { workspaceCapabilityId, targetDir: await localTargetDir(workspace, targetDir) })
    return absoluteInWorkspace(workspace, created)
}

/** Puts workspace files on the OS clipboard so Finder / Explorer can paste them. Local workspaces only. */
export async function clipboardWriteWorkspaceFiles(workspace: string, paths: string[]): Promise<void> {
    if (parseRemoteFilePath(workspace)) throw new Error("clipboard-export-remote-unsupported")
    const { workspaceCapabilityId, relative } = await localMutation(workspace, paths)
    await invoke("clipboard_write_workspace_files", { workspaceCapabilityId, paths: relative })
}

/** Absolute paths currently on the OS clipboard as a file list (empty when none). */
export function clipboardReadFileList(): Promise<string[]> {
    return invoke<string[]>("clipboard_read_file_list")
}

/** Local deletes report progress and can be cancelled by `operation.id`; remote ones report nothing. */
export function fsDelete(
    workspace: string,
    path: string,
    operation?: { id: string; onProgress: (progress: DeleteProgress) => void }
): Promise<void> {
    if (parseRemoteFilePath(workspace) || parseRemoteFilePath(path)) return import("./remoteFiles").then((remote) => remote.deleteRemotePath(workspace, path))
    return localMutation(workspace, [path]).then(({ workspaceCapabilityId, relative }) => {
        const onProgress = new Channel<DeleteProgress>()
        if (operation) onProgress.onmessage = operation.onProgress
        const operationId = operation?.id ?? crypto.randomUUID()
        return invoke("fs_delete", { workspaceCapabilityId, path: relative[0], operationId, onProgress })
    })
}

export function fsDeleteCancel(operationId: string): Promise<void> {
    return invoke("fs_delete_cancel", { operationId })
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

// #57 T3：冷開 workspace 的 git 首載——一趟完成 detect→(status‖branches)，
// 消除 detect 先行寫 State、status/branches 才能發的結構性 waterfall。
// 細粒度 gitStatus/gitBranches 保留給後續 refresh。
export function gitBootstrap(path: string, repositoryPath: string | null = null): Promise<GitBootstrapResult> {
    // `repositoryPath` selects a repository nested in the workspace, relative to it.
    return invokeGit("git_bootstrap", { path, repositoryPath })
}

/** Lists the repositories inside the workspace without running Git. */
export async function gitDiscover(workspacePath: string): Promise<GitDiscovery> {
    if (parseRemoteFilePath(workspacePath)) {
        return import("./remoteGit").then((remote) => remote.invokeRemoteGit<GitDiscovery>("git_discover", { path: workspacePath }))
    }
    const { useWorkspaceStore } = await import("@/state/workspaceStore")
    const state = useWorkspaceStore.getState()
    if (state.workspacePath !== workspacePath || !state.workspaceCapabilityId) throw new Error("workspace-capability-missing")
    return invoke("git_discover", { workspaceCapabilityId: state.workspaceCapabilityId })
}

export function workspaceTrustStatus(path: string): Promise<WorkspaceTrustStatus> {
    if (parseRemoteFilePath(path)) return import("./remoteTrust").then((remote) => remote.remoteTrustStatus(path))
    return invoke("workspace_trust_status", { path })
}

export async function workspaceTrustList(): Promise<TrustedWorkspace[]> {
    const [local, remote] = await Promise.all([invoke<TrustedWorkspace[]>("workspace_trust_list"), import("./remoteTrust").then((remote) => remote.remoteTrustList())])
    return [...local, ...remote]
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

export function gitCommit(repositoryRoot: string, message: string, amendHead?: string): Promise<void> {
    return invokeGit("git_commit_cmd", { repositoryRoot, message, ...(amendHead ? { amendHead } : {}) })
}

export function gitBranches(repositoryRoot: string): Promise<BranchList> {
    return invokeGit("git_branches", { repositoryRoot })
}

// `smart`: JetBrains "Smart Checkout" — stash local changes for the switch
// and restore them afterwards.
export function gitCreateBranch(
    repositoryRoot: string,
    name: string,
    startPoint?: string,
    smart?: boolean
): Promise<GitOperationOutcome> {
    return invokeGit("git_create_branch", { repositoryRoot, name, startPoint: startPoint ?? null, ...(smart ? { smart } : {}) })
}

export function gitCheckout(repositoryRoot: string, name: string, smart?: boolean): Promise<GitOperationOutcome> {
    return invokeGit("git_checkout", { repositoryRoot, name, ...(smart ? { smart } : {}) })
}

export function gitCheckoutDetached(repositoryRoot: string, rev: string, smart?: boolean): Promise<GitOperationOutcome> {
    return invokeGit("git_checkout_detached", { repositoryRoot, rev, ...(smart ? { smart } : {}) })
}

export function gitCherryPick(repositoryRoot: string, hash: string): Promise<void> {
    return invokeGit("git_cherry_pick", { repositoryRoot, hash })
}

export function gitFetch(repositoryRoot: string, background: boolean): Promise<void> {
    return invokeGit("git_fetch_cmd", { repositoryRoot, background })
}

export function gitPull(repositoryRoot: string, mode?: "merge" | "rebase"): Promise<void> {
    return invokeGit("git_pull_cmd", { repositoryRoot, mode: mode ?? null })
}

export function gitPush(repositoryRoot: string, options?: { forceWithLease?: boolean; tags?: boolean }): Promise<void> {
    return invokeGit("git_push_cmd", {
        repositoryRoot,
        forceWithLease: options?.forceWithLease ?? false,
        tags: options?.tags ?? false,
    })
}

export function gitMergeBranch(repositoryRoot: string, name: string): Promise<GitOperationOutcome> {
    return invokeGit("git_merge_branch", { repositoryRoot, name })
}

export function gitRebaseOnto(repositoryRoot: string, upstream: string): Promise<GitOperationOutcome> {
    return invokeGit("git_rebase_onto", { repositoryRoot, upstream })
}

export function gitRenameBranch(repositoryRoot: string, oldName: string, newName: string): Promise<void> {
    return invokeGit("git_rename_branch", { repositoryRoot, oldName, newName })
}

/** Without `force`, an unmerged branch fails with a "not fully merged" error. */
export function gitDeleteBranch(repositoryRoot: string, name: string, force: boolean): Promise<void> {
    return invokeGit("git_delete_branch", { repositoryRoot, name, force })
}

export function gitRevertCommit(repositoryRoot: string, hash: string): Promise<GitOperationOutcome> {
    return invokeGit("git_revert_commit", { repositoryRoot, hash })
}

export function gitResetBranch(repositoryRoot: string, hash: string, mode: GitResetMode): Promise<void> {
    return invokeGit("git_reset_branch", { repositoryRoot, hash, mode })
}

export function gitStashList(repositoryRoot: string): Promise<GitStashEntry[]> {
    return invokeGit("git_stash_list", { repositoryRoot })
}

export function gitStashPush(
    repositoryRoot: string,
    message: string | null,
    includeUntracked: boolean,
    keepIndex: boolean,
): Promise<void> {
    return invokeGit("git_stash_push", { repositoryRoot, message, includeUntracked, keepIndex })
}

export function gitStashApply(repositoryRoot: string, index: number, pop: boolean): Promise<GitOperationOutcome> {
    return invokeGit("git_stash_apply", { repositoryRoot, index, pop })
}

export function gitStashDrop(repositoryRoot: string, index: number): Promise<void> {
    return invokeGit("git_stash_drop", { repositoryRoot, index })
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

export function gitConflictSkip(repositoryRoot: string, op: string): Promise<void> {
    return invokeGit("git_conflict_skip", { repositoryRoot, op })
}

export function gitConflictSides(repositoryRoot: string, path: string): Promise<GitConflictSides> {
    return invokeGit("git_conflict_sides", { repositoryRoot, path })
}

/** Accept one whole side of each conflicted path and mark it resolved. */
export function gitConflictResolve(repositoryRoot: string, paths: string[], side: "ours" | "theirs"): Promise<void> {
    return invokeGit("git_conflict_resolve", { repositoryRoot, paths, side })
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




export function dbListTables(identity: DbConnectionIdentity): Promise<DbTable[]> {
    return invoke("db_list_tables", { identity })
}

export function dbListDatabases(identity: DbConnectionIdentity): Promise<string[]> {
    return invoke("db_list_databases", { identity })
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

export function previewOpenUrl(
    url: string,
    x: number,
    y: number,
    width: number,
    height: number,
    sessionId?: string
): Promise<void> {
    return invoke("preview_open_url", { url, x, y, width, height, sessionId })
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

export function previewBack(sessionId?: string): Promise<void> {
    return invoke("preview_back", { sessionId })
}

export function previewForward(sessionId?: string): Promise<void> {
    return invoke("preview_forward", { sessionId })
}

export function previewNavigationState(sessionId: string): Promise<import("@/state/previewStore").PreviewNativeNavigationSnapshot> {
    return invoke("preview_navigation_state", { sessionId })
}

export function previewReload(): Promise<void> {
    return invoke("preview_reload")
}

export function previewResourceOpen(source: PreviewResourceSource, path: string): Promise<PreviewResourceLease> {
    return invoke("preview_resource_open", { source, path })
}

export function previewResourceClose(id: string): Promise<void> {
    return invoke("preview_resource_close", { id })
}

export function previewInteractions(sessionId: string, bindings: PreviewShortcutBinding[]): Promise<PreviewInteractionSnapshot | null> {
    return invoke("preview_interactions", { sessionId, bindings })
}

export function previewSelectElement(sessionId: string, active: boolean): Promise<void> {
    return invoke("preview_select_element", { sessionId, active })
}
