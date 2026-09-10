import { invoke, sftpListDir } from "./ipc"
import { requestHost } from "./hostIpc"
import type { ConnectionOwner } from "./runtimeIdentity"
import { parseRemoteFilePath, remoteFilePath, sameConnection } from "./runtimeIdentity"
import type { FileNode, OpenFileResult, WorkspaceOpenResult } from "./types"
import { useSshStore } from "@/state/sshStore"
import { Channel } from "@tauri-apps/api/core"
import { emit } from "@tauri-apps/api/event"
import { loadRemoteWorkspaces, rememberRemoteWorkspace } from "@/state/remoteWorkspaceRegistry"
import { useWorkspaceStore } from "@/state/workspaceStore"

type Backend = { kind: "sftp"; sessionId: string } | { kind: "runtime"; owner: ConnectionOwner; capabilityId: string; isCurrent: () => boolean }
interface RemoteWorkspace { hostId: string; root: string; backend: Backend; users?: number; retiring?: boolean; disposed?: boolean }
interface ReadResult { file: OpenFileResult; revision: string | null }
const workspaces = new Map<string, RemoteWorkspace>()
// Revision is bound to the exact backend connection that supplied the buffer.
const revisions = new Map<string, { backend: Backend; revision: string }>()
interface RemoteWatch { uri: string; backend: Extract<Backend, { kind: "runtime" }>; streamId?: string; retryTimer?: ReturnType<typeof setTimeout> }
let activeWatch: RemoteWatch | null = null
let watchGeneration = 0

async function disposeWorkspace(workspace: RemoteWorkspace): Promise<void> {
  if (workspace.disposed) return
  workspace.disposed = true
  const uri = remoteFilePath(workspace.hostId, workspace.root)
  if (workspaces.get(uri) === workspace) workspaces.delete(uri)
  const backend = workspace.backend
  for (const [path, revision] of revisions) if (revision.backend === backend) revisions.delete(path)
  if (activeWatch?.backend === backend) await stopRemoteWatch()
  if (backend.kind === "runtime") {
    await requestHost(backend.owner, { method: "workspaceClose", params: { workspace: backend.capabilityId } }).catch(() => undefined)
  }
}

/** A browser or in-flight operation keeps the exact capability alive until done. */
export function retainRemoteWorkspace(uri: string): () => Promise<void> {
  const { workspace } = resolve(uri)
  workspace.users = (workspace.users ?? 0) + 1
  let released = false
  return async () => {
    if (released) return
    released = true
    workspace.users = (workspace.users ?? 1) - 1
    if (!workspace.users && workspace.retiring) {
      if (useWorkspaceStore.getState().workspacePath === remoteFilePath(workspace.hostId, workspace.root)) workspace.retiring = false
      else await disposeWorkspace(workspace)
    }
  }
}

export async function releaseRemoteWorkspace(uri: string): Promise<void> {
  const workspace = workspaces.get(uri)
  if (!workspace || useWorkspaceStore.getState().workspacePath === uri) return
  workspace.retiring = true
  if (!workspace.users) await disposeWorkspace(workspace)
}

export function forgetRemoteFileRevision(uri: string): void {
  revisions.delete(uri)
}

export async function stopRemoteWatch(): Promise<void> {
  watchGeneration++
  await closeActiveWatch()
}

async function closeActiveWatch(): Promise<void> {
  const previous = activeWatch
  activeWatch = null
  if (previous?.retryTimer) clearTimeout(previous.retryTimer)
  if (previous?.streamId) await invoke("host_stream_close", { owner: previous.backend.owner, streamId: previous.streamId }).catch(() => undefined)
}

async function notifyChanges(workspace: RemoteWorkspace, paths: string[]): Promise<void> {
  const root = remoteFilePath(workspace.hostId, workspace.root)
  const changed = new Set(paths.map((path) => remoteFilePath(workspace.hostId, path, workspace.root)))
  // Directory/coalesced notifications must also reach dirty documents.
  for (const uri of revisions.keys()) if ([...changed].some((parent) => uri === parent || uri.startsWith(parent.replace(/\/$/, "") + "/"))) changed.add(uri)
  await emit("fs:external-change", { workspaceRoot: root, paths: [...changed] })
}

export async function startRemoteWatch(uri: string, attempt = 0): Promise<void> {
  const generation = ++watchGeneration
  await closeActiveWatch()
  if (generation !== watchGeneration) return
  const { workspace } = resolve(uri)
  const backend = workspace.backend
  if (backend.kind !== "runtime") return
  const watch: RemoteWatch = { uri, backend }
  activeWatch = watch
  const retry = () => {
    if (activeWatch !== watch || watch.retryTimer) return
    watch.retryTimer = setTimeout(() => {
      if (activeWatch === watch) void startRemoteWatch(uri, attempt + 1).catch((error) => console.warn("remote watcher reconnect failed", error))
    }, Math.min(30000, 1000 * 2 ** Math.min(attempt, 5)))
  }
  type Message =
    | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload: { type: string; workspaceRoot?: string; paths?: string[] } } }
    | { type: "closed"; owner: ConnectionOwner }
  const channel = new Channel<Message>()
  let closed = false
  channel.onmessage = (message) => {
    if (activeWatch !== watch) return
    try { assertBackend(uri, backend) } catch { closed = true; retry(); return }
    const owner = message.type === "frame" ? message.frame.owner : message.owner
    if (!sameConnection(owner, backend.owner)) return
    if (message.type === "closed") { closed = true; retry(); return }
    const payload = message.frame.payload
    if (message.frame.version !== 1 || payload.type !== "files" || payload.workspaceRoot !== workspace.root || !payload.paths) return
    const paths = payload.paths.filter((path) => path === workspace.root || path.startsWith(workspace.root.replace(/\/$/, "") + "/"))
    void notifyChanges(workspace, paths).catch((error) => console.warn("remote file notification failed", error))
  }
  let opened: { streamId: string }
  try {
    opened = await invoke<{ streamId: string }>("host_stream_open", { owner: backend.owner, config: { kind: "files", path: workspace.root }, onEvent: channel })
  } catch (error) { retry(); throw error }
  try { assertBackend(uri, backend) } catch { closed = true; retry() }
  if (activeWatch !== watch || closed) {
    await invoke("host_stream_close", { owner: backend.owner, streamId: opened.streamId }).catch(() => undefined)
    return
  }
  watch.streamId = opened.streamId
  // Reconcile the opening/retry gap before relying on incremental events.
  await notifyChanges(workspace, [workspace.root])
}

/** Reopen capabilities and compare original revisions before enabling saves. */
export async function reconnectRemoteWorkspaces(owner: ConnectionOwner, isCurrent: () => boolean): Promise<void> {
  for (const [uri, previous] of [...workspaces]) {
    const before = previous.backend
    if (previous.hostId !== owner.hostId || before.kind !== "runtime" || sameConnection(before.owner, owner)) continue
    const opened = await requestHost<WorkspaceOpenResult>(owner, { method: "workspaceOpen", params: { path: previous.root } })
    if (!isCurrent() || workspaces.get(uri) !== previous || opened.canonicalPath !== previous.root) {
      await requestHost(owner, { method: "workspaceClose", params: { workspace: opened.capabilityId } }).catch(() => undefined)
      continue
    }
    const backend: Backend = { kind: "runtime", owner, capabilityId: opened.capabilityId, isCurrent }
    previous.backend = backend
    const workspace = previous
    for (const [path, original] of [...revisions]) {
      if (original.backend !== before) continue
      const relative = parseRemoteFilePath(path)!.path.slice(previous.root.length).replace(/^\//, "")
      const read = await requestHost<ReadResult>(owner, { method: "filesRead", params: { workspace: opened.capabilityId, path: relative } }).catch(() => null)
      if (!isCurrent() || workspaces.get(uri) !== workspace) return
      if (revisions.get(path) === original && read?.revision === original.revision) revisions.set(path, { backend, revision: original.revision })
    }
    if (!isCurrent()) return
    await notifyChanges(workspace, [workspace.root])
    if (activeWatch?.uri === uri) await startRemoteWatch(uri)
  }
}

export async function registerSftpWorkspace(hostId: string, path: string): Promise<string> {
  const sessionId = useSshStore.getState().sessions[hostId]?.sessionId
  if (!sessionId) throw new Error("SSH host is disconnected")
  const listing = await sftpListDir(sessionId, path)
  if (useSshStore.getState().sessions[hostId]?.sessionId !== sessionId) throw new Error("Remote workspace connection changed; response discarded")
  const uri = remoteFilePath(hostId, listing.cwd)
  const previous = workspaces.get(uri)
  if (previous?.backend.kind === "sftp" && previous.backend.sessionId === sessionId) return uri
  const backend: Backend = { kind: "sftp", sessionId }
  const workspace = { hostId, root: listing.cwd, backend }
  workspaces.set(uri, workspace)
  rememberRemoteWorkspace(uri, "sftp")
  // Reopening an SFTP folder is its refresh boundary. Preserve dirty buffers
  // and only rebind revisions whose source bytes are still unchanged.
  if (previous) {
    for (const [path, original] of [...revisions]) {
      if (original.backend !== previous.backend) continue
      const read = await invoke<ReadResult>("sftp_open_file", { sessionId, path: parseRemoteFilePath(path)!.path }).catch(() => null)
      if (useSshStore.getState().sessions[hostId]?.sessionId !== sessionId || workspaces.get(uri) !== workspace) throw new Error("Remote workspace connection changed; response discarded")
      if (revisions.get(path) === original && read?.revision === original.revision) revisions.set(path, { backend, revision: original.revision })
    }
    await notifyChanges(workspace, [workspace.root])
  }
  return uri
}

export async function registerRuntimeWorkspace(owner: ConnectionOwner, path: string, isCurrent: () => boolean): Promise<string> {
  if (!isCurrent()) throw new Error("Remote workspace connection changed; response discarded")
  const existing = workspaces.get(remoteFilePath(owner.hostId, path))
  if (existing?.backend.kind === "runtime" && sameConnection(existing.backend.owner, owner)) {
    existing.retiring = false
    return remoteFilePath(owner.hostId, existing.root)
  }
  const opened = await requestHost<WorkspaceOpenResult>(owner, { method: "workspaceOpen", params: { path } })
  const uri = remoteFilePath(owner.hostId, opened.canonicalPath)
  if (!isCurrent()) {
    await requestHost(owner, { method: "workspaceClose", params: { workspace: opened.capabilityId } }).catch(() => undefined)
    throw new Error("Remote workspace connection changed; response discarded")
  }
  // Canonical aliases or concurrent folder browsers may open the same root.
  // Keep the registered backend so existing dirty buffers retain their revision.
  const current = workspaces.get(uri)
  if (current?.backend.kind === "runtime" && sameConnection(current.backend.owner, owner)) {
    if (current.backend.capabilityId !== opened.capabilityId) await requestHost(owner, { method: "workspaceClose", params: { workspace: opened.capabilityId } }).catch(() => undefined)
    if (!isCurrent() || workspaces.get(uri) !== current) throw new Error("Remote workspace connection changed; response discarded")
    return uri
  }
  workspaces.set(uri, { hostId: owner.hostId, root: opened.canonicalPath, backend: { kind: "runtime", owner, capabilityId: opened.capabilityId, isCurrent } })
  rememberRemoteWorkspace(uri, "runtime")
  return uri
}

/** Revalidate a persisted folder on its exact host; never fall back to local I/O. */
export async function restoreRemoteWorkspace(uri: string): Promise<void> {
  const resource = parseRemoteFilePath(uri)
  if (!resource || resource.workspaceRoot !== resource.path) throw new Error("Reopen this folder to bind its documents to a workspace")
  const existing = workspaces.get(uri)?.backend
  if (existing?.kind === "runtime" && existing.isCurrent()) return
  if (existing?.kind === "sftp" && useSshStore.getState().sessions[resource.hostId]?.sessionId === existing.sessionId) return
  const access = loadRemoteWorkspaces()[uri]
  if (!access) {
    if (workspaces.has(uri)) return
    throw new Error("Reopen this folder to verify its host and access mode")
  }
  let restored: string
  if (access === "sftp") restored = await registerSftpWorkspace(resource.hostId, resource.path)
  else {
    const { useHostStore } = await import("@/state/hostStore")
    const connection = useHostStore.getState().hosts[resource.hostId]?.connection
    if (!connection) throw new Error("Runtime host is disconnected; connect it before opening this folder")
    restored = await registerRuntimeWorkspace(connection.owner, resource.path, () => useHostStore.getState().hosts[resource.hostId]?.connection === connection)
  }
  if (restored !== uri) throw new Error("Workspace canonical root changed; select the folder again")
}

function resolve(uri: string): { workspace: RemoteWorkspace; path: string; relative: string } {
  const resource = parseRemoteFilePath(uri)
  if (!resource) throw new Error("Not a remote resource")
  if (resource.path.split("/").some((part) => part === "." || part === "..") || resource.path.includes("\0")) throw new Error("Invalid remote path")
  if (resource.workspaceRoot === null) throw new Error("Reopen this folder to bind its documents to a workspace")
  const workspace = workspaces.get(remoteFilePath(resource.hostId, resource.workspaceRoot))
  if (!workspace) throw new Error("Reconnect the remote workspace before opening its files")
  if (workspace.backend.kind === "runtime" && !workspace.backend.isCurrent()) throw new Error("Remote workspace connection changed; response discarded")
  if (resource.path !== workspace.root && !resource.path.startsWith(workspace.root.replace(/\/$/, "") + "/")) throw new Error("Remote file is outside its workspace")
  if (workspace.backend.kind === "sftp" && useSshStore.getState().sessions[workspace.hostId]?.sessionId !== workspace.backend.sessionId) {
    throw new Error("Remote workspace connection changed; reopen the folder")
  }
  return { workspace, path: resource.path, relative: resource.path.slice(workspace.root.length).replace(/^\//, "") }
}

function assertBackend(uri: string, backend: Backend): void {
  if (resolve(uri).workspace.backend !== backend) throw new Error("Remote workspace connection changed; response discarded")
}

export function runtimeWorkspaceService(uri: string) {
  const { workspace } = resolve(uri)
  const backend = workspace.backend
  if (backend.kind !== "runtime") throw new Error("This folder provides SFTP file access only")
  return {
    owner: backend.owner,
    capabilityId: backend.capabilityId,
    root: workspace.root,
    uri: remoteFilePath(workspace.hostId, workspace.root),
    assertCurrent: () => assertBackend(uri, backend)
  }
}

export function connectedWorkspaceOwners(): ConnectionOwner[] {
  const owners = new Map<string, ConnectionOwner>()
  for (const workspace of workspaces.values()) if (workspace.backend.kind === "runtime" && workspace.backend.isCurrent()) {
    const owner = workspace.backend.owner
    if ((owners.get(owner.hostId)?.generation ?? -1) < owner.generation) owners.set(owner.hostId, owner)
  }
  return [...owners.values()]
}

function mutationPath(workspaceUri: string, uri: string) {
  const root = resolve(workspaceUri)
  const target = resolve(uri)
  if (root.workspace !== target.workspace || !target.relative) throw new Error("Operation must stay inside its workspace")
  return target
}

export async function createRemotePath(workspaceUri: string, uri: string, directory: boolean): Promise<void> {
  const release = retainRemoteWorkspace(workspaceUri)
  try {
    const { workspace, path, relative } = mutationPath(workspaceUri, uri)
    const backend = workspace.backend
    if (backend.kind === "runtime") await requestHost(backend.owner, { method: "filesCreate", params: { workspace: backend.capabilityId, path: relative, directory } })
    else await invoke(directory ? "sftp_mkdir" : "sftp_create_file", { sessionId: backend.sessionId, path })
    assertBackend(uri, backend)
  } finally { await release() }
}

export async function renameRemotePath(workspaceUri: string, from: string, to: string): Promise<void> {
  const release = retainRemoteWorkspace(workspaceUri)
  try {
    const source = mutationPath(workspaceUri, from)
    const target = mutationPath(workspaceUri, to)
    const backend = source.workspace.backend
    if (backend.kind === "runtime") await requestHost(backend.owner, { method: "filesRename", params: { workspace: backend.capabilityId, from: source.relative, to: target.relative } })
    else await invoke("sftp_rename", { sessionId: backend.sessionId, from: source.path, to: target.path })
    assertBackend(to, backend)
    for (const [uri, opened] of revisions) {
      if (uri === from || uri.startsWith(from + "/")) {
        revisions.delete(uri)
        if (opened.backend === backend) revisions.set(to + uri.slice(from.length), opened)
      }
    }
  } finally { await release() }
}

export async function deleteRemotePath(workspaceUri: string, uri: string): Promise<void> {
  const release = retainRemoteWorkspace(workspaceUri)
  try {
    const { workspace, path, relative } = mutationPath(workspaceUri, uri)
    const backend = workspace.backend
    if (backend.kind === "runtime") await requestHost(backend.owner, { method: "filesDelete", params: { workspace: backend.capabilityId, path: relative } })
    else {
      const parent = path.slice(0, path.lastIndexOf("/")) || "/"
      const listing = await sftpListDir(backend.sessionId, parent)
      const entry = listing.entries.find((entry) => entry.path === path && entry.nameSafe)
      if (!entry) throw new Error("Remote file no longer exists")
      assertBackend(uri, backend)
      await invoke("sftp_remove", { sessionId: backend.sessionId, path, isDir: entry.isDir && !entry.isSymlink })
    }
    assertBackend(uri, backend)
    for (const path of revisions.keys()) if (path === uri || path.startsWith(uri + "/")) revisions.delete(path)
  } finally { await release() }
}

export async function readRemoteBase64(uri: string, maxBytes: number): Promise<{ data: string; size: number }> {
  const release = retainRemoteWorkspace(uri)
  try {
    const { workspace, path, relative } = resolve(uri)
    const backend = workspace.backend
    const result = backend.kind === "runtime"
      ? await requestHost<{ data: string; size: number }>(backend.owner, { method: "filesReadBase64", params: { workspace: backend.capabilityId, path: relative, max_bytes: maxBytes } })
      : await invoke<{ data: string; size: number }>("sftp_read_file_base64", { sessionId: backend.sessionId, path, maxBytes })
    assertBackend(uri, backend)
    return result
  } finally { await release() }
}

export async function openRemoteWorkspace(uri: string): Promise<WorkspaceOpenResult> {
  await restoreRemoteWorkspace(uri)
  const { workspace } = resolve(uri)
  await listRemoteDir(uri)
  return { canonicalPath: remoteFilePath(workspace.hostId, workspace.root), capabilityId: workspace.backend.kind === "runtime" ? workspace.backend.capabilityId : uri }
}

export async function listRemoteDir(uri: string): Promise<FileNode[]> {
  const release = retainRemoteWorkspace(uri)
  try {
    const { workspace, path, relative } = resolve(uri)
    const backend = workspace.backend
    if (backend.kind === "sftp") {
      const listing = await sftpListDir(backend.sessionId, path)
      assertBackend(uri, backend)
      return listing.entries.filter((entry) => entry.nameSafe).map((entry) => ({ name: entry.name, path: remoteFilePath(workspace.hostId, entry.path, workspace.root), isDir: entry.isDir, kind: entry.isSymlink ? "symlink" : entry.isDir ? "directory" : "file" }))
    }
    const entries = await requestHost<FileNode[]>(backend.owner, { method: "filesList", params: { workspace: backend.capabilityId, path: relative } })
    assertBackend(uri, backend)
    return entries.map((entry) => ({ ...entry, path: remoteFilePath(workspace.hostId, `${workspace.root.replace(/\/$/, "")}/${entry.path}`, workspace.root) }))
  } finally { await release() }
}

export async function readRemoteFileSnapshot(uri: string): Promise<{ result: OpenFileResult; accept: () => void }> {
  const release = retainRemoteWorkspace(uri)
  try {
    const { workspace, path, relative } = resolve(uri)
    const backend = workspace.backend
    const result = backend.kind === "sftp"
      ? await invoke<ReadResult>("sftp_open_file", { sessionId: backend.sessionId, path })
      : await requestHost<ReadResult>(backend.owner, { method: "filesRead", params: { workspace: backend.capabilityId, path: relative } })
    assertBackend(uri, backend)
    return {
      result: result.file,
      accept: () => {
        assertBackend(uri, backend)
        if (result.revision) revisions.set(uri, { backend, revision: result.revision })
      }
    }
  } finally { await release() }
}

export async function readRemoteFile(uri: string, recordRevision = true): Promise<OpenFileResult> {
  const snapshot = await readRemoteFileSnapshot(uri)
  if (recordRevision) snapshot.accept()
  return snapshot.result
}

export async function saveRemoteFile(uri: string, content: string): Promise<number> {
  const release = retainRemoteWorkspace(uri)
  try {
    const { workspace, path, relative } = resolve(uri)
    const backend = workspace.backend
    const opened = revisions.get(uri)
    if (!opened || opened.backend !== backend) throw new Error("Compare the remote file after reconnecting before saving")
    let revision: string | null
    if (backend.kind === "sftp") {
      revision = await invoke<string>("sftp_save_file", { sessionId: backend.sessionId, path, content, expectedRevision: opened.revision })
    } else {
      const result = await requestHost<ReadResult>(backend.owner, { method: "filesWrite", params: { workspace: backend.capabilityId, path: relative, content, revision: opened.revision } })
      revision = result.revision
    }
    assertBackend(uri, backend)
    if (revision && revisions.get(uri) === opened) revisions.set(uri, { backend, revision })
    return new TextEncoder().encode(content).length
  } finally { await release() }
}
