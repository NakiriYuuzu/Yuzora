import { useState } from "react"
import { open as openFileDialog } from "@tauri-apps/plugin-dialog"
import { Database, Eye, History, Table2, Trash2, X } from "lucide-react"
import { useTranslation } from "react-i18next"

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
import { relativeTime } from "@/lib/relativeTime"
import type { DbKind, DbOpenConfig } from "@/lib/types"
import { cn } from "@/lib/utils"
import { useDbStore, type SavedDbConnection } from "@/state/dbStore"

/**
 * Database mode nav content (FEAT-1 + F2). Lists persisted connection
 * descriptors — SQLite files and network (PostgreSQL/MSSQL) endpoints — each
 * either connected (active highlighted, closable) or saved-but-offline (click to
 * reconnect). The active connection's tables/views and recent queries hang below.
 * The bottom action opens the "New connection" dialog. Passwords are never
 * stored: a network reconnect re-prompts for one.
 */
export function DatabaseNavContent() {
  const { t } = useTranslation("workbench")
  const connections = useDbStore((s) => s.connections)
  const activeConnId = useDbStore((s) => s.activeConnId)
  const saved = useDbStore((s) => s.saved)
  const tables = useDbStore((s) => s.tables)
  const history = useDbStore((s) => s.history)
  const openConfig = useDbStore((s) => s.openConfig)
  const closeConnection = useDbStore((s) => s.closeConnection)
  const removeSaved = useDbStore((s) => s.removeSaved)
  const setActiveConnection = useDbStore((s) => s.setActiveConnection)
  const openTableQuery = useDbStore((s) => s.openTableQuery)
  const setSql = useDbStore((s) => s.setSql)

  const [dialogOpen, setDialogOpen] = useState(false)
  // Set when reconnecting a network descriptor: the dialog prefills it and asks
  // only for the password.
  const [prefill, setPrefill] = useState<SavedDbConnection | null>(null)

  const activeConn = connections.find((c) => c.connId === activeConnId)
  const activeTables = activeConnId ? (tables[activeConnId] ?? []) : []
  const historyEntries = activeConn ? (history[activeConn.key] ?? []) : []
  const liveByKey = new Map(connections.map((c) => [c.key, c]))

  function openNew() {
    setPrefill(null)
    setDialogOpen(true)
  }

  function handleRowClick(entry: SavedDbConnection) {
    const live = liveByKey.get(entry.id)
    if (live) {
      setActiveConnection(live.connId)
      return
    }
    if (entry.kind === "sqlite" && entry.path) {
      // SQLite reconnects with no secret — open it directly.
      void openConfig({ kind: "sqlite", path: entry.path }).catch((e) =>
        console.error("open database failed", e)
      )
    } else {
      setPrefill(entry)
      setDialogOpen(true)
    }
  }

  return (
    <div className="flex h-full flex-col gap-[10px]">
      {saved.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={Database}
            title={t("database.emptyTitle")}
            description={t("database.emptyDescription")}
          />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-[10px] overflow-y-auto">
          <ul className="flex flex-col gap-[2px]">
            {saved.map((entry) => {
              const live = liveByKey.get(entry.id)
              const isActive = live?.connId === activeConnId
              return (
                <li key={entry.id}>
                  <div
                    className={cn(
                      "group flex h-[34px] items-center gap-[7px] rounded-[8px] px-[8px] text-[12.5px] transition-colors",
                      isActive
                        ? "bg-(--yz-solid) text-(--ink-1)"
                        : "text-(--ink-2) hover:bg-(--yz-hover)"
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => handleRowClick(entry)}
                      title={savedRowTitle(entry)}
                      className="flex min-w-0 flex-1 items-center gap-[7px] text-left"
                    >
                      <Database
                        className={cn(
                          "size-[14px] shrink-0",
                          isActive ? "text-(--yz-accent-ink)" : "text-(--ink-3)"
                        )}
                        aria-hidden="true"
                      />
                      <span className="flex min-w-0 flex-col">
                        <span className={cn("truncate font-medium", !live && "text-(--ink-3)")}>
                          {entry.name}
                        </span>
                        <span className="truncate text-[10.5px] text-(--ink-4)">
                          {kindLabel(entry.kind)}
                          {live ? "" : ` · ${t("database.savedOffline")}`}
                        </span>
                      </span>
                    </button>
                    {live ? (
                      <button
                        type="button"
                        aria-label={t("database.closeConnection", { name: entry.name })}
                        onClick={() => void closeConnection(live.connId)}
                        className="flex size-[18px] shrink-0 items-center justify-center rounded-[5px] text-(--ink-4) opacity-0 transition-opacity group-hover:opacity-100 hover:bg-(--yz-hover) hover:text-(--ink-1)"
                      >
                        <X className="size-[12px]" aria-hidden="true" />
                      </button>
                    ) : (
                      <button
                        type="button"
                        aria-label={t("database.forgetConnection", { name: entry.name })}
                        onClick={() => removeSaved(entry.id)}
                        className="flex size-[18px] shrink-0 items-center justify-center rounded-[5px] text-(--ink-4) opacity-0 transition-opacity group-hover:opacity-100 hover:bg-(--yz-hover) hover:text-(--ink-1)"
                      >
                        <Trash2 className="size-[12px]" aria-hidden="true" />
                      </button>
                    )}
                  </div>
                </li>
              )
            })}
          </ul>

          {activeConnId && (
            <div className="flex min-h-0 flex-col gap-[3px]">
              <span className="px-[8px] text-[10px] font-semibold tracking-[0.08em] text-(--ink-4) uppercase">
                {t("database.tablesHeading")}
              </span>
              {activeTables.length === 0 ? (
                <p className="px-[8px] py-[4px] text-[12px] text-(--ink-4)">{t("database.noTables")}</p>
              ) : (
                <ul className="flex flex-col gap-[1px]">
                  {activeTables.map((tbl) => {
                    const Icon = tbl.kind === "view" ? Eye : Table2
                    return (
                      <li key={tbl.name}>
                        <button
                          type="button"
                          onClick={() => void openTableQuery(tbl.name)}
                          title={`SELECT * FROM ${tbl.name}`}
                          className="flex h-[26px] w-full items-center gap-[7px] rounded-[7px] px-[8px] text-left text-[12.5px] text-(--ink-2) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                        >
                          <Icon className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                          <span className="truncate font-mono">{tbl.name}</span>
                        </button>
                      </li>
                    )
                  })}
                </ul>
              )}
            </div>
          )}

          {activeConn && historyEntries.length > 0 && (
            <div className="flex min-h-0 flex-col gap-[3px]">
              <span className="px-[8px] text-[10px] font-semibold tracking-[0.08em] text-(--ink-4) uppercase">
                {t("database.recentQueriesHeading")}
              </span>
              <ul className="flex flex-col gap-[1px]">
                {historyEntries.map((entry, i) => (
                  <li key={`${entry.ranAt}-${i}`}>
                    <button
                      type="button"
                      onClick={() => setSql(entry.sql)}
                      title={entry.sql}
                      className="flex h-[26px] w-full items-center gap-[7px] rounded-[7px] px-[8px] text-left text-[12.5px] text-(--ink-2) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                    >
                      {entry.ok ? (
                        <History className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                      ) : (
                        <span
                          role="img"
                          aria-label={t("database.historyFailed")}
                          title={entry.error}
                          className="size-[6px] shrink-0 rounded-full bg-(--destructive)"
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate font-mono">
                        {entry.sql.split("\n")[0]}
                      </span>
                      <span className="shrink-0 font-mono text-[10px] text-(--ink-4)">
                        {relativeTime(Math.floor(entry.ranAt / 1000))}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      <DashedActionButton label={t("database.newConnection")} onClick={openNew} />

      <NewConnectionDialog
        key={prefill?.id ?? "new"}
        open={dialogOpen}
        onOpenChange={(next) => {
          setDialogOpen(next)
          if (!next) setPrefill(null)
        }}
        prefill={prefill}
      />
    </div>
  )
}

function kindLabel(kind: DbKind): string {
  return kind === "sqlite" ? "SQLite" : kind === "postgres" ? "PostgreSQL" : "MSSQL"
}

function savedRowTitle(s: SavedDbConnection): string {
  return s.kind === "sqlite"
    ? (s.path ?? s.name)
    : `${s.user ?? ""}@${s.host ?? ""}:${s.port ?? ""}/${s.database ?? ""}`
}

function defaultPort(kind: DbKind): number {
  return kind === "postgres" ? 5432 : 1433
}

function NewConnectionDialog({
  open,
  onOpenChange,
  prefill
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  prefill: SavedDbConnection | null
}) {
  const { t } = useTranslation("workbench")
  const openConfig = useDbStore((s) => s.openConfig)
  const isReconnect = prefill != null
  const [kind, setKind] = useState<DbKind>(prefill?.kind ?? "sqlite")
  const [path, setPath] = useState(prefill?.path ?? "")
  const [host, setHost] = useState(prefill?.host ?? "")
  const [port, setPort] = useState(prefill ? String(prefill.port ?? defaultPort(prefill.kind)) : String(defaultPort("sqlite")))
  const [database, setDatabase] = useState(prefill?.database ?? "")
  const [user, setUser] = useState(prefill?.user ?? "")
  const [password, setPassword] = useState("")
  const [ssl, setSsl] = useState(prefill?.ssl ?? false)
  const [trustCert, setTrustCert] = useState(prefill?.trustCert ?? false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function pickKind(next: DbKind) {
    setKind(next)
    setError(null)
    // Move the port to the new engine's default (network engines only).
    if (next !== "sqlite") setPort(String(defaultPort(next)))
  }

  const portNum = Number.parseInt(port, 10)
  const canSave =
    !busy &&
    (kind === "sqlite"
      ? path.trim().length > 0
      : host.trim().length > 0 &&
        database.trim().length > 0 &&
        user.trim().length > 0 &&
        password.length > 0 &&
        Number.isFinite(portNum) &&
        portNum > 0 &&
        portNum <= 65535)

  async function browseSqlite() {
    try {
      const selected = await openFileDialog({
        multiple: false,
        filters: [
          { name: "SQLite", extensions: ["sqlite", "db", "sqlite3"] },
          { name: t("database.allFilesFilter"), extensions: ["*"] }
        ]
      })
      if (typeof selected === "string") setPath(selected)
    } catch {
      // Picker cancel/failure — keep any manually typed path.
    }
  }

  async function submit() {
    if (!canSave) return
    const config: DbOpenConfig =
      kind === "sqlite"
        ? { kind: "sqlite", path: path.trim() }
        : kind === "postgres"
          ? {
              kind: "postgres",
              host: host.trim(),
              port: portNum,
              database: database.trim(),
              user: user.trim(),
              password,
              ssl
            }
          : {
              kind: "mssql",
              host: host.trim(),
              port: portNum,
              database: database.trim(),
              user: user.trim(),
              password,
              trustCert
            }
    setBusy(true)
    setError(null)
    try {
      await openConfig(config)
      onOpenChange(false)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
          <DialogTitle>
            {isReconnect ? t("database.reconnectDialogTitle") : t("database.newConnectionDialogTitle")}
          </DialogTitle>
          <DialogDescription>
            {isReconnect
              ? t("database.reconnectDialogDescription", { name: prefill!.name })
              : t("database.newConnectionDialogDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-[10px]">
          {!isReconnect && (
            <Field label={t("database.fieldEngine")}>
              <div className="flex gap-[6px]">
                <KindChoice label="SQLite" active={kind === "sqlite"} onClick={() => pickKind("sqlite")} />
                <KindChoice label="PostgreSQL" active={kind === "postgres"} onClick={() => pickKind("postgres")} />
                <KindChoice label="MSSQL" active={kind === "mssql"} onClick={() => pickKind("mssql")} />
              </div>
            </Field>
          )}

          {kind === "sqlite" ? (
            <Field label={t("database.fieldFile")}>
              <div className="flex gap-[6px]">
                <Input
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                  placeholder={t("database.filePlaceholder")}
                  className="flex-1"
                />
                <Button type="button" variant="outline" onClick={() => void browseSqlite()}>
                  {t("database.browse")}
                </Button>
              </div>
            </Field>
          ) : (
            <>
              <div className="flex gap-[8px]">
                <Field label={t("database.fieldHost")} className="flex-1">
                  <Input
                    value={host}
                    onChange={(e) => setHost(e.target.value)}
                    placeholder={t("database.hostPlaceholder")}
                    readOnly={isReconnect}
                    autoFocus={!isReconnect}
                  />
                </Field>
                <Field label={t("database.fieldPort")} className="w-[84px]">
                  <Input
                    value={port}
                    onChange={(e) => setPort(e.target.value)}
                    inputMode="numeric"
                    readOnly={isReconnect}
                  />
                </Field>
              </div>
              <Field label={t("database.fieldDatabase")}>
                <Input
                  value={database}
                  onChange={(e) => setDatabase(e.target.value)}
                  placeholder={t("database.databasePlaceholder")}
                  readOnly={isReconnect}
                />
              </Field>
              <Field label={t("database.fieldUser")}>
                <Input
                  value={user}
                  onChange={(e) => setUser(e.target.value)}
                  placeholder={t("database.userPlaceholder")}
                  readOnly={isReconnect}
                />
              </Field>
              <Field label={t("database.fieldPassword")}>
                <Input
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  autoFocus={isReconnect}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault()
                      void submit()
                    }
                  }}
                />
              </Field>
              <label className="flex items-center gap-[7px] text-[12px] text-(--ink-2)">
                <input
                  type="checkbox"
                  checked={kind === "postgres" ? ssl : trustCert}
                  onChange={(e) =>
                    kind === "postgres" ? setSsl(e.target.checked) : setTrustCert(e.target.checked)
                  }
                />
                {kind === "postgres" ? t("database.useSsl") : t("database.trustCert")}
              </label>
            </>
          )}

          {error && (
            <p className="rounded-[6px] bg-(--danger-soft) px-[8px] py-[6px] font-mono text-[11px] whitespace-pre-wrap text-(--destructive)">
              {error}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("database.cancel")}
          </Button>
          <Button onClick={() => void submit()} disabled={!canSave}>
            {t("database.connect")}
          </Button>
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

function KindChoice({
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
        "flex-1 rounded-[8px] border px-[8px] py-[6px] text-[12px] font-medium transition-colors",
        active
          ? "border-(--yz-accent) bg-(--yz-solid) text-(--ink-1)"
          : "border-(--line-1) text-(--ink-3) hover:bg-(--yz-hover)"
      )}
    >
      {label}
    </button>
  )
}
