import { useEffect, useRef, useState } from "react"
import { open as openFileDialog, save as saveDialog } from "@tauri-apps/plugin-dialog"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import {
  ArrowUp,
  Download,
  File as FileIcon,
  Folder,
  FolderPlus,
  FolderSync,
  Pencil,
  RefreshCw,
  ShieldCheck,
  TerminalSquare,
  Trash2,
  Upload,
  X
} from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { cn } from "@/lib/utils"
import { listDir } from "@/lib/ipc"
import type { FileNode, SftpEntry } from "@/lib/types"
import { SshTerminalSession } from "@/terminal/SshTerminalSession"
import {
  physicalPointInRect,
  useSftpStore,
  type RemotePaneState,
  type TransferState
} from "@/state/sftpStore"
import { useSshStore, type SshSessionState } from "@/state/sshStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

/**
 * SSH mode main region (FEAT-2). The SSH tab hosts the xterm terminal for the
 * active host's live session; a dropped session swaps in a reconnect banner.
 * SFTP stays a placeholder — file transfer is out of the MVP scope.
 */
export function SshPanel() {
  const { t } = useTranslation("panels")
  const activeTab = useSftpStore((s) => s.activeTab)
  const setActiveTab = useSftpStore((s) => s.setActiveTab)
  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      <Tabs
        value={activeTab}
        onValueChange={(v) => setActiveTab(v as "ssh" | "sftp")}
        className="min-h-0 flex-1 gap-0"
      >
        <div className="flex shrink-0 items-center justify-center border-b border-(--line-1) px-[10px] py-[10px]">
          <TabsList aria-label={t("sshPanel.tabsAriaLabel")} className="group-data-horizontal/tabs:h-[26px] rounded-(--r-pill) bg-(--paper-2) p-[3px]">
            <TabsTrigger
              value="sftp"
              className="rounded-(--r-pill) px-[12px] text-[11.5px] font-medium data-active:bg-(--yz-solid) data-active:shadow-(--shadow-xs)"
            >
              {t("sshPanel.sftpTab")}
            </TabsTrigger>
            <TabsTrigger
              value="ssh"
              className="rounded-(--r-pill) px-[12px] text-[11.5px] font-medium data-active:bg-(--yz-solid) data-active:shadow-(--shadow-xs)"
            >
              {t("sshPanel.sshTab")}
            </TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="sftp" className="min-h-0 flex-1">
          <SftpTabContent />
        </TabsContent>

        <TabsContent value="ssh" className="min-h-0 flex-1">
          <SshTabContent />
        </TabsContent>
      </Tabs>
    </div>
  )
}

type ConnectedSession = SshSessionState & { sessionId: string }

function isConnected(session: SshSessionState | undefined): session is ConnectedSession {
  return session?.status === "connected" && session.sessionId !== null
}

/**
 * Every host that is currently connected keeps its SshTerminalSession mounted
 * (just hidden via `visibility` while inactive) instead of only rendering the
 * active host's session. Unmounting on every host switch would dispose xterm
 * and wipe its scrollback each time the sidebar selection changed — this way
 * a background host keeps its buffer (and keeps receiving ssh://data) while
 * out of view, matching how the pty TerminalDrawer keeps every pane mounted.
 */
function SshTabContent() {
  const activeHostId = useSshStore((s) => s.activeHostId)
  const sessions = useSshStore((s) => s.sessions)
  const beginConnect = useSshStore((s) => s.beginConnect)

  const connectedSessions = Object.values(sessions).filter(isConnected)
  const activeSession = activeHostId ? sessions[activeHostId] : undefined

  return (
    <div className="relative h-full min-h-0 w-full overflow-hidden">
      {connectedSessions.map((session) => {
        const isActive = session.hostId === activeHostId
        return (
          <div
            key={session.hostId}
            aria-hidden={!isActive}
            style={{ visibility: isActive ? "visible" : "hidden" }}
            className="absolute inset-0 flex min-h-0 flex-col"
          >
            <FingerprintBar
              key={session.sessionId}
              fingerprint={session.fingerprint}
              knownHost={session.knownHost}
            />
            <div className="min-h-0 flex-1">
              <SshTerminalSession sessionId={session.sessionId} active={isActive} />
            </div>
          </div>
        )
      })}

      {!isConnected(activeSession) ? (
        <NonTerminalState
          activeHostId={activeHostId}
          session={activeSession}
          onReconnect={beginConnect}
        />
      ) : null}
    </div>
  )
}

function NonTerminalState({
  activeHostId,
  session,
  onReconnect
}: {
  activeHostId: string | null
  session: SshSessionState | undefined
  onReconnect: (id: string) => void
}) {
  const { t } = useTranslation("panels")

  if (!activeHostId || !session) {
    return (
      <div className="absolute inset-0 flex items-center justify-center">
        <EmptyState
          icon={TerminalSquare}
          title={t("sshPanel.noSessionTitle")}
          description={t("sshPanel.noSessionDescription")}
        />
      </div>
    )
  }

  if (session.status === "connecting") {
    return (
      <div className="absolute inset-0 flex items-center justify-center">
        <p className="text-[12.5px] text-(--ink-3)">{t("sshPanel.connecting")}</p>
      </div>
    )
  }

  const message =
    session.status === "error"
      ? (session.error ?? t("sshPanel.connectionFailed"))
      : t("sshPanel.sessionEnded")

  return (
    <div className="absolute inset-0 flex flex-col items-center justify-center gap-[12px] px-[24px] text-center">
      <p className="max-w-[420px] text-[12.5px] whitespace-pre-wrap text-(--ink-3)">{message}</p>
      <Button onClick={() => onReconnect(activeHostId)}>{t("sshPanel.reconnect")}</Button>
    </div>
  )
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

// Parent of a native local path (macOS/Linux "/"): "/a/b/c" → "/a/b", "/a" → "/".
function localParent(path: string): string {
  const trimmed = path.replace(/\/+$/, "")
  const slash = trimmed.lastIndexOf("/")
  if (slash <= 0) return "/"
  return trimmed.slice(0, slash)
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
          icon={FolderSync}
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
  const remote = useSftpStore((s) => s.remote[hostId])
  const listRemote = useSftpStore((s) => s.listRemote)
  const upload = useSftpStore((s) => s.upload)
  const transfers = useSftpStore((s) => s.transfers)
  const [dragOver, setDragOver] = useState(false)
  const remotePaneRef = useRef<HTMLDivElement>(null)

  // Load the remote home directory the first time the browser mounts for a host.
  useEffect(() => {
    if (!remote) void listRemote(hostId, "")
  }, [hostId, remote, listRemote])

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
            for (const p of payload.paths) void upload(hostId, p)
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
  }, [hostId, upload])

  const hostTransfers = Object.entries(transfers).filter(([, tr]) => tr.hostId === hostId)

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1">
        <LocalPane hostId={hostId} />
        <div className="w-px shrink-0 bg-(--line-1)" />
        <RemotePane
          hostId={hostId}
          remote={remote}
          dragOver={dragOver}
          paneRef={remotePaneRef}
        />
      </div>
      {hostTransfers.length > 0 ? <TransfersStrip entries={hostTransfers} /> : null}
    </div>
  )
}

function LocalPane({ hostId }: { hostId: string }) {
  const { t } = useTranslation("panels")
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const upload = useSftpStore((s) => s.upload)
  const [cwd, setCwd] = useState<string | null>(workspacePath)
  const [entries, setEntries] = useState<FileNode[]>([])
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (cwd === null && workspacePath) setCwd(workspacePath)
  }, [workspacePath, cwd])

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
  }, [cwd])

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PaneHeader title={t("sshPanel.sftpLocalTitle")} path={cwd ?? ""}>
        <IconButton
          label={t("sshPanel.sftpUp")}
          onClick={() => cwd && setCwd(localParent(cwd))}
          disabled={!cwd || cwd === "/"}
          icon={ArrowUp}
        />
      </PaneHeader>
      <div className="min-h-0 flex-1 overflow-y-auto px-[4px] py-[4px]">
        {!cwd ? (
          <PaneNote text={t("sshPanel.sftpLocalNoWorkspace")} />
        ) : error ? (
          <PaneNote text={error} />
        ) : entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpEmptyDir")} />
        ) : (
          entries.map((node) => (
            <div
              key={node.path}
              className="group flex h-[26px] items-center gap-[6px] rounded-[6px] px-[6px] text-[12px] hover:bg-(--yz-hover)"
            >
              <button
                type="button"
                onClick={() => node.isDir && setCwd(node.path)}
                className="flex min-w-0 flex-1 items-center gap-[6px] text-left"
              >
                {node.isDir ? (
                  <Folder className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                ) : (
                  <FileIcon className="size-[13px] shrink-0 text-(--ink-4)" aria-hidden="true" />
                )}
                <span className="truncate text-(--ink-2)">{node.name}</span>
              </button>
              {!node.isDir ? (
                <IconButton
                  label={t("sshPanel.sftpUploadFile", { name: node.name })}
                  onClick={() => void upload(hostId, node.path)}
                  icon={Upload}
                  hoverOnly
                />
              ) : null}
            </div>
          ))
        )}
      </div>
    </div>
  )
}

function RemotePane({
  hostId,
  remote,
  dragOver,
  paneRef
}: {
  hostId: string
  remote: RemotePaneState | undefined
  dragOver: boolean
  paneRef: React.RefObject<HTMLDivElement | null>
}) {
  const { t } = useTranslation("panels")
  const listRemote = useSftpStore((s) => s.listRemote)
  const navigateUp = useSftpStore((s) => s.navigateUp)
  const navigateInto = (entry: SftpEntry) => void listRemote(hostId, entry.path)
  const mkdir = useSftpStore((s) => s.mkdir)
  const rename = useSftpStore((s) => s.rename)
  const remove = useSftpStore((s) => s.remove)
  const upload = useSftpStore((s) => s.upload)
  const download = useSftpStore((s) => s.download)

  async function pickAndUpload() {
    const selected = await openFileDialog({ multiple: true })
    if (!selected) return
    const paths = Array.isArray(selected) ? selected : [selected]
    for (const p of paths) void upload(hostId, p)
  }

  async function promptMkdir() {
    const name = window.prompt(t("sshPanel.sftpNewFolderPrompt"))?.trim()
    if (name) await mkdir(hostId, name)
  }

  async function promptRename(entry: SftpEntry) {
    const name = window.prompt(t("sshPanel.sftpRenamePrompt"), entry.name)?.trim()
    if (name && name !== entry.name) await rename(hostId, entry, name)
  }

  async function confirmRemove(entry: SftpEntry) {
    const text = entry.isDir
      ? t("sshPanel.sftpDeleteDirConfirm", { name: entry.name })
      : t("sshPanel.sftpDeleteConfirm", { name: entry.name })
    if (window.confirm(text)) await remove(hostId, entry)
  }

  async function pickAndDownload(entry: SftpEntry) {
    const target = await saveDialog({ defaultPath: entry.name })
    if (typeof target === "string") void download(hostId, entry, target)
  }

  return (
    <div
      ref={paneRef}
      data-testid="sftp-remote-pane"
      className={cn(
        "flex min-h-0 flex-1 flex-col",
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
      </PaneHeader>
      <div className="min-h-0 flex-1 overflow-y-auto px-[4px] py-[4px]">
        {remote?.error ? (
          <PaneNote text={remote.error} />
        ) : remote?.loading && remote.entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpLoading")} />
        ) : remote && remote.entries.length === 0 ? (
          <PaneNote text={t("sshPanel.sftpEmptyDir")} />
        ) : (
          remote?.entries.map((entry) => (
            <div
              key={entry.path}
              className="group flex h-[26px] items-center gap-[6px] rounded-[6px] px-[6px] text-[12px] hover:bg-(--yz-hover)"
            >
              <button
                type="button"
                onClick={() => entry.isDir && navigateInto(entry)}
                title={entry.name}
                className="flex min-w-0 flex-1 items-center gap-[6px] text-left"
              >
                {entry.isDir ? (
                  <Folder className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
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
              <div className="flex shrink-0 items-center gap-[2px] opacity-0 transition-opacity group-hover:opacity-100">
                {!entry.isDir ? (
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
          ))
        )}
      </div>
    </div>
  )
}

function TransfersStrip({ entries }: { entries: [string, TransferState][] }) {
  const { t } = useTranslation("panels")
  const clearTransfer = useSftpStore((s) => s.clearTransfer)
  return (
    <div className="flex shrink-0 flex-col gap-[2px] border-t border-(--line-1) bg-(--paper-2) px-[10px] py-[6px]">
      {entries.map(([id, tr]) => {
        const pct = tr.total > 0 ? Math.min(100, Math.round((tr.transferred / tr.total) * 100)) : null
        const label = tr.error
          ? t("sshPanel.sftpTransferFailed")
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
              <button
                type="button"
                aria-label={t("sshPanel.sftpClearTransfer")}
                onClick={() => clearTransfer(id)}
                className="flex size-[14px] shrink-0 items-center justify-center rounded-[3px] text-(--ink-4) hover:bg-(--yz-hover) hover:text-(--ink-1)"
              >
                <X className="size-[10px]" aria-hidden="true" />
              </button>
            ) : null}
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
