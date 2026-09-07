import { useEffect, useRef, useState } from "react"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import {
  ArrowUp,
  Download,
  File as FileIcon,
  FileSymlink,
  Folder,
  FolderPlus,
  Pencil,
  RefreshCw,
  Server,
  ShieldCheck,
  Trash2,
  Upload,
  X
} from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { requestTextInputDialog } from "@/state/textInputDialogStore"
import {
  listDir,
  sftpPickDownloadDestination,
  sftpPickSelectedPath,
  sftpPickTree
} from "@/lib/ipc"
import {
  isSafeLeafName,
  localPathParent,
  relativePathWithin,
  samePathIdentity,
  workspacePathForDisplay
} from "@/lib/paths"
import type { FileNode, SftpEntry, SftpUploadSource } from "@/lib/types"
import {
  physicalPointInRect,
  useSftpStore,
  type RemotePaneState,
  type TransferState
} from "@/state/sftpStore"
import { useSshStore, type SshSessionState } from "@/state/sshStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

/** SFTP is a transfer surface; it never creates an independent SSH shell. */
export function SftpPanel() {
  const activeHostId = useSshStore((s) => s.activeHostId)
  const session = useSshStore((s) => activeHostId ? s.sessions[activeHostId] : undefined)
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
    {session?.sessionId && <FingerprintBar key={session.sessionId} fingerprint={session.fingerprint} knownHost={session.knownHost} />}
    <SftpTabContent />
  </div>
}

type ConnectedSession = SshSessionState & { sessionId: string }

function isConnected(session: SshSessionState | undefined): session is ConnectedSession {
  return session?.status === "connected" && session.sessionId !== null
}

/**
 * Host-key notice (TOFU). check_server_key now pins the key in a known-hosts
 * store: a changed key is rejected before auth, so this bar reflects a verified
 * state — "known host" when the key matched the pinned fingerprint, or a
 * first-seen notice otherwise. The fingerprint is still shown for eyeballing.
 * Keyed on sessionId so a reconnect (fresh fingerprint) un-dismisses it.
 */
function FingerprintBar({
  fingerprint,
  knownHost
}: {
  fingerprint: string | null
  knownHost: boolean
}) {
  const { t } = useTranslation("panels")
  const [dismissed, setDismissed] = useState(false)
  if (!fingerprint || dismissed) return null
  return (
    <div className="flex shrink-0 items-center gap-[8px] border-b border-(--line-1) bg-(--paper-2) px-[10px] py-[6px]">
      <ShieldCheck className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
      <span className="min-w-0 flex-1 truncate text-[11px] text-(--ink-3)">
        {knownHost ? t("sshPanel.knownHostNotice") : t("sshPanel.fingerprintNotice")}{" "}
        <span className="font-mono text-(--ink-2)">{fingerprint}</span>
      </span>
      <button
        type="button"
        aria-label={t("sshPanel.dismissFingerprintNotice")}
        onClick={() => setDismissed(true)}
        className="flex size-[16px] shrink-0 items-center justify-center rounded-[4px] text-(--ink-4) hover:bg-(--yz-hover) hover:text-(--ink-1)"
      >
        <X className="size-[11px]" aria-hidden="true" />
      </button>
    </div>
  )
}

// --- SFTP browser (F5) -----------------------------------------------------

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ["KB", "MB", "GB", "TB"]
  let value = bytes / 1024
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i += 1
  }
  return `${value.toFixed(value >= 10 || i === 0 ? 0 : 1)} ${units[i]}`
}

// Parent of a native local path. Supports POSIX, drive letters, UNC, and
// Windows verbatim forms. Remote SFTP paths stay POSIX and must not use this.
function localParent(path: string): string {
  return localPathParent(path)
}

/**
 * The SFTP tab: a "Local | Remote" split browser over the active host's live
 * session. The remote side reuses the SSH session's SFTP subsystem; the local
 * side reuses fs_service. OS files dropped onto the remote pane upload into the
 * remote cwd (Phase 1).
 */
function SftpTabContent() {
  const { t } = useTranslation("panels")
  const activeHostId = useSshStore((s) => s.activeHostId)
  const sessions = useSshStore((s) => s.sessions)
  const beginConnect = useSshStore((s) => s.beginConnect)
  const activeSession = activeHostId ? sessions[activeHostId] : undefined

  if (!activeHostId || !isConnected(activeSession)) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-[12px] px-[24px] text-center">
        <EmptyState
          icon={Server}
          title={t("sshPanel.sftpDisconnectedTitle")}
          description={t("sshPanel.sftpDisconnectedDescription")}
        />
        {activeHostId ? (
          <Button onClick={() => beginConnect(activeHostId)}>{t("sshPanel.sftpConnect")}</Button>
        ) : null}
      </div>
    )
  }

  return <SftpBrowser hostId={activeHostId} />
}

function SftpBrowser({ hostId }: { hostId: string }) {
  const sessionId = useSshStore((s) => s.sessions[hostId]?.sessionId)
  const remote = useSftpStore((s) => s.remote[hostId])
  const listRemote = useSftpStore((s) => s.listRemote)
  const download = useSftpStore((s) => s.download)
  const transfers = useSftpStore((s) => s.transfers)
  const [dragOver, setDragOver] = useState(false)
  const remotePaneRef = useRef<HTMLDivElement>(null)

  // Pane-internal pointer drag (P5): state drives the target highlights, the
  // ref gives the window-level handlers the current value without re-binding.
  const [drag, setDrag] = useState<SftpDragState | null>(null)
  const [hover, setHover] = useState<SftpHoverTarget | null>(null)
  const dragRef = useRef<SftpDragState | null>(null)
  dragRef.current = drag
  // Pressing a file row selects it (row highlight); a stale path simply stops
  // matching after navigation, so no explicit clearing is needed.
  const [selected, setSelected] = useState<{ pane: SftpPaneKind; path: string } | null>(null)
  // Bumped after a drop-download completes so LocalPane re-lists in place.
  const [localRefreshTick, setLocalRefreshTick] = useState(0)

  function beginRowDrag(
    kind: SftpPaneKind,
    payload: { path: string; name: string; size: number },
    event: React.PointerEvent
  ) {
    if (event.button !== 0) return
    // The row's hover action buttons (upload/download/rename/delete) bubble
    // their pointerdown up here — a press-and-wiggle on an action button must
    // stay a button click, never become a drag. Only the marked action area is
    // excluded: the filename itself (also a <button>) covers most of the row
    // and must remain a drag handle.
    if ((event.target as HTMLElement).closest("[data-sftp-row-actions]")) return
    // Pressing a file row selects it (highlight), holding + moving drags it.
    setSelected({ pane: kind, path: payload.path })
    setDrag({ kind, ...payload, active: false, startX: event.clientX, startY: event.clientY })
  }

  // Remote file dropped on the local pane / a local folder row → download, with
  // the same overwrite guard as before (the button path goes through saveDialog,
  // which confirms natively; a drop must not silently clobber a local file).
  // confirm comes from plugin-dialog, matching the repo's confirm convention
  // (contextMenuStore) rather than relying on wry's window.confirm.
  async function acceptRemoteToLocalDrop(payload: SftpDragState, _target: SftpHoverTarget) {
    if (!isSafeLeafName(payload.name)) return
    const grant = await sftpPickDownloadDestination(payload.name)
    if (!grant) return
    await download(
      hostId,
      { name: payload.name, path: payload.path, isDir: false, isSymlink: false, size: payload.size },
      { capabilityId: grant.id, leaf: grant.leaf }
    )
    setLocalRefreshTick((tick) => tick + 1)
  }

  const dragInProgress = drag !== null
  useEffect(() => {
    if (!dragInProgress) return
    const finish = () => {
      document.body.style.userSelect = ""
      document.body.style.cursor = ""
      setDrag(null)
      setHover(null)
    }
    const onMove = (event: PointerEvent) => {
      const current = dragRef.current
      if (!current) return
      if (!current.active) {
        const travelled = Math.hypot(event.clientX - current.startX, event.clientY - current.startY)
        if (travelled < SFTP_DRAG_ACTIVATE_PX) return
        document.body.style.userSelect = "none"
        document.body.style.cursor = "grabbing"
        setDrag({ ...current, active: true })
        return
      }
      const target = sftpDropTargetAt(event.clientX, event.clientY)
      // Mutually exclusive directions: hovering the drag's own pane is a no-op.
      setHover(target && target.pane !== current.kind ? target : null)
    }
    const onUp = (event: PointerEvent) => {
      const current = dragRef.current
      finish()
      if (!current?.active) return
      const target = sftpDropTargetAt(event.clientX, event.clientY)
      if (!target || target.pane === current.kind) return
      if (current.kind === "local") {
        // Local file → remote pane / folder row: undefined destDir falls back
        // to the remote cwd inside the store.
        void uploadFromLocalPath(hostId, current.path, target.dirPath ?? undefined)
      } else {
        void acceptRemoteToLocalDrop(current, target)
      }
    }
    const onCancel = () => finish()
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") finish()
    }
    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onCancel)
    window.addEventListener("keydown", onKey)
    return () => {
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onCancel)
      window.removeEventListener("keydown", onKey)
      document.body.style.userSelect = ""
      document.body.style.cursor = ""
    }
  }, [dragInProgress])

  // Every new session gets a fresh directory snapshot.
  useEffect(() => {
    if (sessionId) void listRemote(hostId, useSftpStore.getState().remote[hostId]?.cwd ?? "")
  }, [hostId, sessionId, listRemote])

  // Phase 1: OS file drop onto the remote pane uploads into the remote cwd. The
  // event reports a PhysicalPosition, so physicalPointInRect divides by the DPR
  // before hit-testing against the pane's logical-px bounding rect.
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let disposed = false
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type === "leave") {
          setDragOver(false)
          return
        }
        const rect = remotePaneRef.current?.getBoundingClientRect()
        const inside = rect
          ? physicalPointInRect(payload.position, rect, window.devicePixelRatio || 1)
          : false
        if (payload.type === "drop") {
          setDragOver(false)
          if (inside) {
            for (const p of payload.paths) void uploadFromLocalPath(hostId, p)
          }
        } else {
          setDragOver(inside)
        }
      })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [hostId])

  const hostTransfers = Object.entries(transfers).filter(([, tr]) => tr.hostId === hostId)

  const activeDragKind = drag?.active ? drag.kind : null

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <div className="flex min-h-0 min-w-0 flex-1">
        <LocalPane
          hostId={hostId}
          dragKind={activeDragKind}
          hover={hover}
          selectedPath={selected?.pane === "local" ? selected.path : null}
          onFileDragStart={beginRowDrag}
          refreshTick={localRefreshTick}
        />
        <div className="w-px shrink-0 bg-(--line-1)" />
        <RemotePane
          hostId={hostId}
          remote={remote}
          dragOver={dragOver}
          paneRef={remotePaneRef}
          dragKind={activeDragKind}
          hover={hover}
          selectedPath={selected?.pane === "remote" ? selected.path : null}
          onFileDragStart={beginRowDrag}
        />
      </div>
      {hostTransfers.length > 0 ? <TransfersStrip entries={hostTransfers} /> : null}
    </div>
  )
}

// Pane-internal drags (P5) are pointer-driven, NOT HTML5 drag & drop: Tauri's
// drag-drop layer (dragDropEnabled, required for OS file drops) swallows HTML5
// drops inside the webview on macOS, so internal transfers track pointer events
// and hit-test their drop targets themselves. Direction stays mutually
// exclusive in the drop handler (a target pane equal to the drag's source kind
// is a no-op), and OS file drops keep flowing through onDragDropEvent — the two
// channels never touch by construction (pointer events bypass the drag layer).
type SftpPaneKind = "local" | "remote"

interface SftpDragState {
  kind: SftpPaneKind
  path: string
  name: string
  size: number
  /** True once the pointer travelled past the activation threshold. */
  active: boolean
  startX: number
  startY: number
}

interface SftpHoverTarget {
  pane: SftpPaneKind
  /** Directory row under the pointer, or null for the pane body itself. */
  dirPath: string | null
  /** The pane's current directory, read from the pane's data attribute. */
  paneCwd: string
}

// A press only becomes a drag after moving this many logical px, so plain
// clicks (open folder, hover buttons) keep working on draggable rows.
const SFTP_DRAG_ACTIVATE_PX = 4

// Resolve the drop target under the pointer from DOM data attributes — the
// rendered rows/panes are the single source of truth for cwd and dir paths.
function sftpDropTargetAt(x: number, y: number): SftpHoverTarget | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null
  if (!el) return null
  const paneEl = el.closest<HTMLElement>("[data-sftp-drop-pane]")
  if (!paneEl) return null
  const pane = paneEl.dataset.sftpDropPane as SftpPaneKind
  const paneCwd = paneEl.dataset.sftpPaneCwd ?? ""
  const dirEl = el.closest<HTMLElement>("[data-sftp-dir-pane]")
  if (dirEl && dirEl.dataset.sftpDirPane === pane) {
    return { pane, dirPath: dirEl.dataset.sftpDirPath ?? null, paneCwd }
  }
  return { pane, dirPath: null, paneCwd }
}

function workspaceUploadSource(
  workspacePath: string,
  workspaceId: string,
  localPath: string
): SftpUploadSource | null {
  if (parseRemoteFilePath(workspacePath) || parseRemoteFilePath(localPath)) return null
  const relativePath = relativePathWithin(workspacePath, localPath)
  if (!relativePath) return null
  return { kind: "workspace", workspaceId, relativePath }
}

async function uploadFromLocalPath(hostId: string, localPath: string, destDir?: string) {
  const { workspacePath, workspaceCapabilityId } = useWorkspaceStore.getState()
  const workspaceSource = workspacePath && workspaceCapabilityId
    ? workspaceUploadSource(workspacePath, workspaceCapabilityId, localPath)
    : null
  if (workspaceSource) {
    await useSftpStore.getState().upload(hostId, workspaceSource, destDir)
    return
  }
  // Outside the workspace the renderer has no authority to mint a path
  // capability. Native picker grants are the only non-workspace source.
}

function localNodeKind(node: FileNode): NonNullable<FileNode["kind"]> {
  return node.kind ?? (node.isDir ? "directory" : "file")
}

function canUploadLocalNode(node: FileNode): boolean {
  const kind = localNodeKind(node)
  return kind === "file" && isSafeLeafName(node.name)
}

function canTransferRemoteEntry(entry: SftpEntry): boolean {
  return !entry.isDir && !entry.isSymlink && entry.nameSafe !== false && isSafeLeafName(entry.name)
}

const SFTP_ERROR_KEYS: Record<string, string> = {
  "unsafe-leaf-name": "sshPanel.sftpErrorUnsafeLeaf",
  "unsafe-relative-path": "sshPanel.sftpErrorUnsafeLeaf",
  "symlink-rejected": "sshPanel.sftpErrorSymlink",
  "reparse-rejected": "sshPanel.sftpErrorReparse",
  "not-a-regular-file": "sshPanel.sftpErrorNotRegular",
  "destination-busy": "sshPanel.sftpErrorDestinationBusy",
  "selected-path-expired": "sshPanel.sftpErrorCapabilityExpired",
  "selected-path-missing": "sshPanel.sftpErrorCapabilityExpired",
  "path-not-utf8": "sshPanel.sftpErrorPathNotUtf8",
  "path-escape": "sshPanel.sftpErrorWorkspaceEscape",
  "file-conflict": "sshPanel.sftpErrorConflict",
  "sftp-atomic-replace-unavailable": "sshPanel.sftpErrorAtomicUnavailable",
  "sftp-transfer-cancelled": "sshPanel.sftpCancelled",
  "sftp-transfer-limit": "sshPanel.sftpErrorTransferLimit",
  "sftp-connection-changed": "sshPanel.sftpErrorConnectionChanged",
  "sftp-source-changed": "sshPanel.sftpErrorSourceChanged",
  "sftp-folder-conflict": "sshPanel.sftpFolderConflict",
  "sftp-tree-unsupported-entry": "sshPanel.sftpTreeUnsupported",
  "sftp-tree-entry-limit": "sshPanel.sftpTreeLimit",
  "sftp-tree-depth-limit": "sshPanel.sftpTreeLimit",
  "sftp-tree-size-limit": "sshPanel.sftpTreeLimit",
  "sftp-tree-selection-expired": "sshPanel.sftpErrorCapabilityExpired"
}

function localizedSftpError(t: (key: string) => string, error: string): string {
  const code = error.trim()
  if (code.includes("sftp-partial-staging:")) return `${t("sshPanel.sftpTreePartial")} ${code.split("sftp-partial-staging:")[1].trim()}`
  const key = SFTP_ERROR_KEYS[code.split(":")[0]]
  return key ? t(key) : t("sshPanel.sftpTransferFailed")
}

function LocalPane({
  hostId,
  dragKind,
  hover,
  selectedPath,
  onFileDragStart,
  refreshTick
}: {
  hostId: string
  dragKind: SftpPaneKind | null
  hover: SftpHoverTarget | null
  selectedPath: string | null
  onFileDragStart: (
    kind: SftpPaneKind,
    payload: { path: string; name: string; size: number },
    event: React.PointerEvent
  ) => void
  refreshTick: number
}) {
  const { t } = useTranslation("panels")
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const nativeWorkspacePath = workspacePath && !parseRemoteFilePath(workspacePath) ? workspacePath : null
  const [cwd, setCwd] = useState<string | null>(nativeWorkspacePath)
  const [entries, setEntries] = useState<FileNode[]>([])
  const [error, setError] = useState<string | null>(null)

  // This pane only receives remote-sourced drags (download direction).
  const paneDropActive = dragKind === "remote" && hover?.pane === "local" && hover.dirPath === null
  const rowDropPath = dragKind === "remote" && hover?.pane === "local" ? hover.dirPath : null

  useEffect(() => {
    setCwd(nativeWorkspacePath)
    setEntries([])
  }, [nativeWorkspacePath])

  // refreshTick: bumped by SftpBrowser after a drop-download completes so the
  // freshly downloaded file appears without leaving the directory.
  useEffect(() => {
    if (!cwd) return
    let cancelled = false
    setError(null)
    void listDir(cwd)
      .then((nodes) => {
        if (!cancelled) setEntries(nodes)
      })
      .catch((e) => {
        if (!cancelled) setError(String(e))
      })
    return () => {
      cancelled = true
    }
  }, [cwd, refreshTick])

  const parent = cwd ? localParent(cwd) : null
  const atRoot = Boolean(cwd && parent && samePathIdentity(cwd, parent))

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <PaneHeader title={t("sshPanel.sftpLocalTitle")} path={workspacePathForDisplay(cwd ?? "")}>
        <IconButton
          label={t("sshPanel.sftpUp")}
          onClick={() => parent && setCwd(parent)}
          disabled={!cwd || !parent || atRoot}
          icon={ArrowUp}
        />
      </PaneHeader>
      <ScrollArea
        data-testid="sftp-local-pane-body"
        data-sftp-drop-pane="local"
        data-sftp-pane-cwd={cwd ?? ""}
        className={cn(
          "min-h-0 flex-1",
          paneDropActive && "bg-(--yz-hover) ring-2 ring-inset ring-(--yz-accent)"
        )}
        viewportClassName="px-[4px] py-[4px]"
      >
        {!cwd ? (
          <PaneNote text={t("sshPanel.sftpLocalNoWorkspace")} />
        ) : error ? (
          <PaneNote text={error} />
        ) : entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpEmptyDir")} />
        ) : (
          entries.map((node) => {
            const kind = localNodeKind(node)
            const uploadable = canUploadLocalNode(node)
            return (
            <div
              key={node.path}
              onPointerDown={
                uploadable
                  ? (event) =>
                      onFileDragStart("local", { path: node.path, name: node.name, size: 0 }, event)
                  : undefined
              }
              data-sftp-dir-pane={node.isDir ? "local" : undefined}
              data-sftp-dir-path={node.isDir ? node.path : undefined}
              className={cn(
                "group flex h-[26px] items-center gap-[6px] rounded-[6px] px-[6px] text-[12px] hover:bg-(--yz-hover)",
                selectedPath === node.path && "bg-(--yz-active) shadow-(--shadow-xs)",
                rowDropPath === node.path && "bg-(--yz-hover) ring-1 ring-inset ring-(--yz-accent)"
              )}
            >
              <button
                type="button"
                onClick={() => node.isDir && setCwd(node.path)}
                title={kind === "symlink" ? t("sshPanel.sftpSymlinkEntry", { name: node.name }) : node.name}
                className="flex min-w-0 flex-1 items-center gap-[6px] text-left"
              >
                {node.isDir ? (
                  <Folder className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                ) : kind === "symlink" ? (
                  <FileSymlink className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
                ) : (
                  <FileIcon className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
                )}
                <span className="truncate text-(--ink-2)">{node.name}</span>
              </button>
              {uploadable ? (
                <span data-sftp-row-actions className="flex shrink-0 items-center">
                  <IconButton
                    label={t("sshPanel.sftpUploadFile", { name: node.name })}
                    onClick={() => {
                      void uploadFromLocalPath(hostId, node.path)
                    }}
                    icon={Upload}
                    hoverOnly
                  />
                </span>
              ) : null}
            </div>
            )
          })
        )}
      </ScrollArea>
    </div>
  )
}

function RemotePane({
  hostId,
  remote,
  dragOver,
  paneRef,
  dragKind,
  hover,
  selectedPath,
  onFileDragStart
}: {
  hostId: string
  remote: RemotePaneState | undefined
  dragOver: boolean
  paneRef: React.RefObject<HTMLDivElement | null>
  dragKind: SftpPaneKind | null
  hover: SftpHoverTarget | null
  selectedPath: string | null
  onFileDragStart: (
    kind: SftpPaneKind,
    payload: { path: string; name: string; size: number },
    event: React.PointerEvent
  ) => void
}) {
  const { t } = useTranslation("panels")
  const listRemote = useSftpStore((s) => s.listRemote)
  const navigateUp = useSftpStore((s) => s.navigateUp)
  const navigateInto = (entry: SftpEntry) => void listRemote(hostId, entry.path)
  const mkdir = useSftpStore((s) => s.mkdir)
  const rename = useSftpStore((s) => s.rename)
  const remove = useSftpStore((s) => s.remove)
  const download = useSftpStore((s) => s.download)

  // This pane only receives local-sourced drags (upload direction).
  const paneDropActive = dragKind === "local" && hover?.pane === "remote" && hover.dirPath === null
  const rowDropPath = dragKind === "local" && hover?.pane === "remote" ? hover.dirPath : null

  async function pickAndUpload() {
    const grants = await sftpPickSelectedPath()
    if (!grants.length) return
    for (const grant of grants) {
      void useSftpStore.getState().upload(
        hostId,
        { kind: "selected", capabilityId: grant.id, name: grant.leaf }
      )
    }
  }

  async function pickAndUploadFolder() {
    const sessionId = useSshStore.getState().sessions[hostId]?.sessionId
    const cwd = remote?.cwd
    if (!sessionId || !cwd) return
    const grant = await sftpPickTree("upload")
    if (!grant) return
    const name = await requestTextInputDialog({ title: t("sshPanel.sftpUploadFolder"), label: t("sshPanel.sftpNewDestination"), initialValue: grant.leaf, confirmLabel: t("sshPanel.sftpUpload") })
    if (!name || !isSafeLeafName(name) || useSshStore.getState().sessions[hostId]?.sessionId !== sessionId) return
    await useSftpStore.getState().transferTree(hostId, grant.id, "upload", cwd, name)
  }

  async function promptMkdir() {
    const name = await requestTextInputDialog({
      title: t("sshPanel.sftpNewFolderPrompt"),
      label: t("sshPanel.sftpNameLabel"),
      confirmLabel: t("sshPanel.sftpCreate")
    })
    if (name && isSafeLeafName(name)) await mkdir(hostId, name)
  }

  async function promptRename(entry: SftpEntry) {
    const name = await requestTextInputDialog({
      title: t("sshPanel.sftpRenamePrompt"),
      label: t("sshPanel.sftpNameLabel"),
      initialValue: entry.name,
      confirmLabel: t("sshPanel.sftpRenameAction")
    })
    if (name && name !== entry.name && isSafeLeafName(name)) await rename(hostId, entry, name)
  }

  async function confirmRemove(entry: SftpEntry) {
    const text = entry.isDir
      ? t("sshPanel.sftpDeleteDirConfirm", { name: entry.name })
      : t("sshPanel.sftpDeleteConfirm", { name: entry.name })
    const accepted = await requestAppConfirmation({
      title: t("sshPanel.sftpDeleteTitle"),
      description: text,
      kind: "warning",
      destructive: true
    })
    if (accepted) await remove(hostId, entry)
  }

  async function pickAndDownload(entry: SftpEntry) {
    if (entry.isDir) {
      const sessionId = useSshStore.getState().sessions[hostId]?.sessionId
      const name = await requestTextInputDialog({ title: t("sshPanel.sftpDownloadFolder"), label: t("sshPanel.sftpNewDestination"), initialValue: entry.name, confirmLabel: t("sshPanel.sftpDownloadFolder") })
      if (!name || !isSafeLeafName(name)) return
      const grant = await sftpPickTree("download", name)
      if (!grant || useSshStore.getState().sessions[hostId]?.sessionId !== sessionId) return
      await useSftpStore.getState().transferTree(hostId, grant.id, "download", entry.path, name)
      return
    }
    const grant = await sftpPickDownloadDestination(entry.name)
    if (!grant) return
    void download(hostId, entry, { capabilityId: grant.id, leaf: grant.leaf })
  }

  return (
    <div
      ref={paneRef}
      data-testid="sftp-remote-pane"
      data-yuzora-os-file-drop-target="sftp-upload"
      className={cn(
        "flex min-h-0 min-w-0 flex-1 flex-col",
        dragOver && "bg-(--yz-hover) ring-2 ring-inset ring-(--yz-accent)"
      )}
    >
      <PaneHeader title={t("sshPanel.sftpRemoteTitle")} path={remote?.cwd ?? ""}>
        <IconButton
          label={t("sshPanel.sftpUp")}
          onClick={() => void navigateUp(hostId)}
          disabled={!remote || remote.cwd === "/" || remote.cwd === ""}
          icon={ArrowUp}
        />
        <IconButton
          label={t("sshPanel.sftpRefresh")}
          onClick={() => remote && void listRemote(hostId, remote.cwd)}
          disabled={!remote}
          icon={RefreshCw}
        />
        <IconButton
          label={t("sshPanel.sftpNewFolder")}
          onClick={() => void promptMkdir()}
          disabled={!remote}
          icon={FolderPlus}
        />
        <IconButton
          label={t("sshPanel.sftpUpload")}
          onClick={() => void pickAndUpload()}
          disabled={!remote}
          icon={Upload}
        />
        <IconButton label={t("sshPanel.sftpUploadFolder")} onClick={() => void pickAndUploadFolder()} disabled={!remote} icon={Folder} />
      </PaneHeader>
      <ScrollArea
        data-testid="sftp-remote-pane-body"
        data-sftp-drop-pane="remote"
        data-sftp-pane-cwd={remote?.cwd ?? ""}
        className={cn(
          "min-h-0 flex-1",
          paneDropActive && "bg-(--yz-hover) ring-2 ring-inset ring-(--yz-accent)"
        )}
        viewportClassName="px-[4px] py-[4px]"
      >
        {remote?.error ? (
          <PaneNote text={remote.error} />
        ) : remote?.loading && remote.entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpLoading")} />
        ) : remote && remote.entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpEmptyDir")} />
        ) : (
          remote?.entries.map((entry) => {
            const transferable = canTransferRemoteEntry(entry)
            return (
            <div
              key={entry.path}
              onPointerDown={
                transferable
                  ? (event) =>
                      onFileDragStart(
                        "remote",
                        { path: entry.path, name: entry.name, size: entry.size },
                        event
                      )
                  : undefined
              }
              data-sftp-dir-pane={entry.isDir ? "remote" : undefined}
              data-sftp-dir-path={entry.isDir ? entry.path : undefined}
              className={cn(
                "group flex h-[26px] items-center gap-[6px] rounded-[6px] px-[6px] text-[12px] hover:bg-(--yz-hover)",
                selectedPath === entry.path && "bg-(--yz-active) shadow-(--shadow-xs)",
                rowDropPath === entry.path && "bg-(--yz-hover) ring-1 ring-inset ring-(--yz-accent)"
              )}
            >
              <button
                type="button"
                onClick={() => entry.isDir && navigateInto(entry)}
                title={
                  entry.isSymlink
                    ? t("sshPanel.sftpSymlinkEntry", { name: entry.name })
                    : entry.nameSafe === false
                      ? t("sshPanel.sftpUnsafeEntry", { name: entry.name })
                      : entry.name
                }
                className="flex min-w-0 flex-1 items-center gap-[6px] text-left"
              >
                {entry.isDir ? (
                  <Folder className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                ) : entry.isSymlink ? (
                  <FileSymlink className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
                ) : (
                  <FileIcon className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
                )}
                <span className="truncate text-(--ink-2)">{entry.name}</span>
                {!entry.isDir ? (
                  <span className="shrink-0 text-[10.5px] text-(--ink-4)">
                    {formatBytes(entry.size)}
                  </span>
                ) : null}
              </button>
              <div
                data-sftp-row-actions
                className="flex shrink-0 items-center gap-[2px] opacity-0 transition-opacity group-hover:opacity-100"
              >
                {transferable || (entry.isDir && !entry.isSymlink && entry.nameSafe !== false && isSafeLeafName(entry.name)) ? (
                  <IconButton
                    label={t("sshPanel.sftpDownload", { name: entry.name })}
                    onClick={() => void pickAndDownload(entry)}
                    icon={Download}
                  />
                ) : null}
                <IconButton
                  label={t("sshPanel.sftpRename", { name: entry.name })}
                  onClick={() => void promptRename(entry)}
                  icon={Pencil}
                />
                <IconButton
                  label={t("sshPanel.sftpDelete", { name: entry.name })}
                  onClick={() => void confirmRemove(entry)}
                  icon={Trash2}
                />
              </div>
            </div>
            )
          })
        )}
      </ScrollArea>
    </div>
  )
}

function TransfersStrip({ entries }: { entries: [string, TransferState][] }) {
  const { t } = useTranslation("panels")
  const clearTransfer = useSftpStore((s) => s.clearTransfer)
  const cancelTransfer = useSftpStore((s) => s.cancelTransfer)
  return (
    <div className="flex shrink-0 flex-col gap-[2px] border-t border-(--line-1) bg-(--paper-2) px-[10px] py-[6px]">
      {entries.map(([id, tr]) => {
        const pct = tr.total > 0 ? Math.min(100, Math.round((tr.transferred / tr.total) * 100)) : null
        const label = tr.error
          ? localizedSftpError(t, tr.error)
          : tr.cancelling && !tr.done
            ? t("sshPanel.sftpCancelling")
          : tr.done
            ? t("sshPanel.sftpTransferDone")
            : tr.direction === "upload"
              ? t("sshPanel.sftpUploading")
              : t("sshPanel.sftpDownloading")
        return (
          <div key={id} className="flex items-center gap-[8px] text-[11px]">
            {tr.direction === "upload" ? (
              <Upload className="size-[11px] shrink-0 text-(--ink-4)" aria-hidden="true" />
            ) : (
              <Download className="size-[11px] shrink-0 text-(--ink-4)" aria-hidden="true" />
            )}
            <span className="min-w-0 flex-1 truncate text-(--ink-2)">{tr.name}</span>
            <span className={cn("shrink-0", tr.error ? "text-(--destructive)" : "text-(--ink-4)")}>
              {label}
              {pct !== null && !tr.done && !tr.error ? ` ${pct}%` : ""}
            </span>
            {tr.done || tr.error ? (
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("sshPanel.sftpClearTransfer")}
                onClick={() => clearTransfer(id)}
              >
                <X aria-hidden="true" />
              </Button>
            ) : <Button variant="ghost" size="xs" disabled={tr.cancelling} onClick={() => void cancelTransfer(id)}>{t("sshPanel.sftpCancel")}</Button>}
          </div>
        )
      })}
    </div>
  )
}

function PaneHeader({
  title,
  path,
  children
}: {
  title: string
  path: string
  children: React.ReactNode
}) {
  return (
    <div className="flex shrink-0 items-center gap-[6px] border-b border-(--line-1) px-[8px] py-[6px]">
      <span className="shrink-0 text-[11px] font-semibold text-(--ink-3)">{title}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-(--ink-4)" title={path}>
        {path}
      </span>
      <div className="flex shrink-0 items-center gap-[2px]">{children}</div>
    </div>
  )
}

function PaneNote({ text }: { text: string }) {
  return <p className="px-[6px] py-[8px] text-[11.5px] text-(--ink-4)">{text}</p>
}

function IconButton({
  label,
  onClick,
  icon: Icon,
  disabled,
  hoverOnly
}: {
  label: string
  onClick: () => void
  icon: typeof ArrowUp
  disabled?: boolean
  hoverOnly?: boolean
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "flex size-[20px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1) disabled:opacity-40",
        hoverOnly && "opacity-0 group-hover:opacity-100"
      )}
    >
      <Icon className="size-[13px]" aria-hidden="true" />
    </button>
  )
}
