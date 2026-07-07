import { useState } from "react"
import { useTranslation } from "react-i18next"
import { open as openFileDialog } from "@tauri-apps/plugin-dialog"
import { Check, KeyRound, Lock, Pencil, Server, Trash2, X } from "lucide-react"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import {
  useSshStore,
  type NewSshHost,
  type SshHost,
  type SshSessionStatus
} from "@/state/sshStore"

/**
 * SSH mode nav content (FEAT-2). Lists the persisted host book with a live
 * connection badge; a click connects (password → prompt, key → connect). The
 * "New host" action opens a form dialog. Secrets are never stored — only the
 * connection descriptor lives in localStorage (see sshStore).
 */
export function SshNavContent() {
  const { t } = useTranslation("workbench")
  const hosts = useSshStore((s) => s.hosts)
  const sessions = useSshStore((s) => s.sessions)
  const activeHostId = useSshStore((s) => s.activeHostId)
  const pendingAuthHostId = useSshStore((s) => s.pendingAuthHostId)
  const beginConnect = useSshStore((s) => s.beginConnect)
  const removeHost = useSshStore((s) => s.removeHost)

  const [formOpen, setFormOpen] = useState(false)
  const [editingHost, setEditingHost] = useState<SshHost | null>(null)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)

  const pendingHost = pendingAuthHostId ? hosts.find((h) => h.id === pendingAuthHostId) : undefined

  function openNewHost() {
    setConfirmDeleteId(null)
    setEditingHost(null)
    setFormOpen(true)
  }

  function openEditHost(target: SshHost) {
    setConfirmDeleteId(null)
    setEditingHost(target)
    setFormOpen(true)
  }

  function handleFormOpenChange(next: boolean) {
    setFormOpen(next)
    if (!next) setEditingHost(null)
  }

  return (
    <div className="flex h-full flex-col gap-[10px]">
      {hosts.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={Server}
            title={t("ssh.emptyTitle")}
            description={t("ssh.emptyDescription")}
          />
        </div>
      ) : (
        <ul className="flex min-h-0 flex-1 flex-col gap-[2px] overflow-y-auto">
          {hosts.map((host) => {
            const status = sessions[host.id]?.status
            const isActive = host.id === activeHostId
            return (
              <li key={host.id}>
                <div
                  onContextMenu={contextMenuHandler("sshhost", {
                    hostId: host.id,
                    host: `${host.user}@${host.host}:${host.port}`
                  })}
                  className={cn(
                    "group flex h-[34px] items-center gap-[7px] rounded-[8px] px-[8px] text-[12.5px] transition-colors",
                    isActive
                      ? "bg-(--yz-solid) text-(--ink-1)"
                      : "text-(--ink-2) hover:bg-(--yz-hover)"
                  )}
                >
                  <button
                    type="button"
                    onClick={() => beginConnect(host.id)}
                    title={`${host.user}@${host.host}:${host.port}`}
                    className="flex min-w-0 flex-1 items-center gap-[7px] text-left"
                  >
                    {host.authKind === "key" ? (
                      <KeyRound
                        className={cn(
                          "size-[14px] shrink-0",
                          isActive ? "text-(--yz-accent-ink)" : "text-(--ink-3)"
                        )}
                        aria-hidden="true"
                      />
                    ) : (
                      <Lock
                        className={cn(
                          "size-[14px] shrink-0",
                          isActive ? "text-(--yz-accent-ink)" : "text-(--ink-3)"
                        )}
                        aria-hidden="true"
                      />
                    )}
                    <span className="flex min-w-0 flex-col">
                      <span className="truncate font-medium">{host.name}</span>
                      <span className="truncate text-[10.5px] text-(--ink-4)">
                        {host.user}@{host.host}
                      </span>
                    </span>
                  </button>
                  <StatusBadge status={status} error={sessions[host.id]?.error ?? null} />
                  {confirmDeleteId === host.id ? (
                    <div className="flex shrink-0 items-center gap-[2px]">
                      <button
                        type="button"
                        aria-label={t("ssh.confirmRemove", { name: host.name })}
                        onClick={() => {
                          setConfirmDeleteId(null)
                          removeHost(host.id)
                        }}
                        className="flex size-[18px] items-center justify-center rounded-[5px] text-(--destructive) transition-colors hover:bg-(--danger-soft)"
                      >
                        <Check className="size-[12px]" aria-hidden="true" />
                      </button>
                      <button
                        type="button"
                        aria-label={t("ssh.cancelRemove")}
                        onClick={() => setConfirmDeleteId(null)}
                        className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                      >
                        <X className="size-[12px]" aria-hidden="true" />
                      </button>
                    </div>
                  ) : (
                    <div className="flex shrink-0 items-center gap-[2px] opacity-0 transition-opacity group-hover:opacity-100">
                      <button
                        type="button"
                        aria-label={t("ssh.editHost", { name: host.name })}
                        onClick={() => openEditHost(host)}
                        className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                      >
                        <Pencil className="size-[12px]" aria-hidden="true" />
                      </button>
                      <button
                        type="button"
                        aria-label={t("ssh.removeHost", { name: host.name })}
                        onClick={() => setConfirmDeleteId(host.id)}
                        className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                      >
                        <Trash2 className="size-[12px]" aria-hidden="true" />
                      </button>
                    </div>
                  )}
                </div>
              </li>
            )
          })}
        </ul>
      )}

      <DashedActionButton label={t("ssh.newHost")} onClick={openNewHost} />

      <NewHostDialog
        key={editingHost?.id ?? "new"}
        open={formOpen}
        onOpenChange={handleFormOpenChange}
        editingHost={editingHost}
      />
      {pendingHost ? <PasswordPromptDialog host={pendingHost} /> : null}
    </div>
  )
}

function StatusBadge({
  status,
  error
}: {
  status: SshSessionStatus | undefined
  error: string | null
}) {
  const { t } = useTranslation("workbench")
  if (!status) return null
  const map: Record<SshSessionStatus, { label: string; className: string }> = {
    connecting: { label: t("ssh.statusConnecting"), className: "bg-(--amber-soft) text-(--ink-2)" },
    connected: { label: t("ssh.statusConnected"), className: "bg-(--yz-hover) text-(--term-ok)" },
    error: { label: t("ssh.statusError"), className: "bg-(--danger-soft) text-(--destructive)" },
    disconnected: { label: t("ssh.statusOffline"), className: "bg-(--yz-hover) text-(--ink-4)" }
  }
  const { label, className } = map[status]
  return (
    <span
      title={status === "error" && error ? error : undefined}
      className={cn(
        "shrink-0 rounded-(--r-pill) px-[7px] py-[1px] text-[10px] font-medium",
        className
      )}
    >
      {label}
    </span>
  )
}

const DEFAULT_PORT = 22

function NewHostDialog({
  open,
  onOpenChange,
  editingHost
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  editingHost?: SshHost | null
}) {
  const { t } = useTranslation("workbench")
  const addHost = useSshStore((s) => s.addHost)
  const updateHost = useSshStore((s) => s.updateHost)
  const isEdit = editingHost != null
  const [name, setName] = useState(editingHost?.name ?? "")
  const [host, setHost] = useState(editingHost?.host ?? "")
  const [port, setPort] = useState(editingHost ? String(editingHost.port) : String(DEFAULT_PORT))
  const [user, setUser] = useState(editingHost?.user ?? "")
  const [authKind, setAuthKind] = useState<"password" | "key">(editingHost?.authKind ?? "password")
  const [keyPath, setKeyPath] = useState(editingHost?.keyPath ?? "")

  function reset() {
    setName("")
    setHost("")
    setPort(String(DEFAULT_PORT))
    setUser("")
    setAuthKind("password")
    setKeyPath("")
  }

  const portNum = Number.parseInt(port, 10)
  const canSave =
    host.trim().length > 0 &&
    user.trim().length > 0 &&
    Number.isFinite(portNum) &&
    portNum > 0 &&
    portNum <= 65535 &&
    (authKind === "password" || keyPath.trim().length > 0)

  function save() {
    if (!canSave) return
    const input: NewSshHost = {
      name: name.trim() || host.trim(),
      host: host.trim(),
      port: portNum,
      user: user.trim(),
      authKind,
      ...(authKind === "key" ? { keyPath: keyPath.trim() } : {})
    }
    if (editingHost) {
      updateHost(editingHost.id, input)
    } else {
      addHost(input)
    }
    reset()
    onOpenChange(false)
  }

  async function browseKey() {
    try {
      const selected = await openFileDialog({ multiple: false })
      if (typeof selected === "string") setKeyPath(selected)
    } catch {
      // Picker cancel/failure — keep the manually typed path.
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) reset()
        onOpenChange(next)
      }}
    >
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
          <DialogTitle>
            {isEdit ? t("ssh.editHostDialogTitle") : t("ssh.newHostDialogTitle")}
          </DialogTitle>
          <DialogDescription>{t("ssh.newHostDialogDescription")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-[10px]">
          <Field label={t("ssh.fieldName")}>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={t("ssh.namePlaceholder")} />
          </Field>
          <div className="flex gap-[8px]">
            <Field label={t("ssh.fieldHost")} className="flex-1">
              <Input
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder={t("ssh.hostPlaceholder")}
                autoFocus
              />
            </Field>
            <Field label={t("ssh.fieldPort")} className="w-[84px]">
              <Input
                value={port}
                onChange={(e) => setPort(e.target.value)}
                inputMode="numeric"
              />
            </Field>
          </div>
          <Field label={t("ssh.fieldUser")}>
            <Input value={user} onChange={(e) => setUser(e.target.value)} placeholder={t("ssh.userPlaceholder")} />
          </Field>
          <Field label={t("ssh.fieldAuthentication")}>
            <div className="flex gap-[6px]">
              <AuthChoice
                label={t("ssh.authPassword")}
                active={authKind === "password"}
                onClick={() => setAuthKind("password")}
              />
              <AuthChoice
                label={t("ssh.authKeyFile")}
                active={authKind === "key"}
                onClick={() => setAuthKind("key")}
              />
            </div>
          </Field>
          {authKind === "key" ? (
            <Field label={t("ssh.fieldPrivateKeyPath")}>
              <div className="flex gap-[6px]">
                <Input
                  value={keyPath}
                  onChange={(e) => setKeyPath(e.target.value)}
                  placeholder={t("ssh.keyPathPlaceholder")}
                  className="flex-1"
                />
                <Button type="button" variant="outline" onClick={() => void browseKey()}>
                  {t("ssh.browse")}
                </Button>
              </div>
            </Field>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("ssh.cancel")}
          </Button>
          <Button onClick={save} disabled={!canSave}>
            {isEdit ? t("ssh.save") : t("ssh.add")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function PasswordPromptDialog({ host }: { host: SshHost }) {
  const { t } = useTranslation("workbench")
  const connect = useSshStore((s) => s.connect)
  const cancelPendingAuth = useSshStore((s) => s.cancelPendingAuth)
  const [password, setPassword] = useState("")

  function submit() {
    void connect(host.id, password)
    setPassword("")
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) {
          setPassword("")
          cancelPendingAuth()
        }
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle>{t("ssh.passwordDialogTitle")}</DialogTitle>
          <DialogDescription>
            {t("ssh.passwordDialogDescription", { user: host.user, host: host.host })}
          </DialogDescription>
        </DialogHeader>
        <Input
          aria-label={t("ssh.passwordAriaLabel")}
          type="password"
          autoFocus
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault()
              submit()
            }
          }}
        />
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => {
              setPassword("")
              cancelPendingAuth()
            }}
          >
            {t("ssh.cancel")}
          </Button>
          <Button onClick={submit}>{t("ssh.connect")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Field({
  label,
  className,
  children
}: {
  label: string
  className?: string
  children: React.ReactNode
}) {
  return (
    <label className={cn("flex flex-col gap-[4px]", className)}>
      <span className="text-[11px] font-medium text-(--ink-3)">{label}</span>
      {children}
    </label>
  )
}

function AuthChoice({
  label,
  active,
  onClick
}: {
  label: string
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex-1 rounded-[8px] border px-[10px] py-[6px] text-[12px] font-medium transition-colors",
        active
          ? "border-(--yz-accent) bg-(--yz-solid) text-(--ink-1)"
          : "border-(--line-1) text-(--ink-3) hover:bg-(--yz-hover)"
      )}
    >
      {label}
    </button>
  )
}
