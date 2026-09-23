import { useEffect, useRef, useState } from "react"
import { open as openFileDialog } from "@tauri-apps/plugin-dialog"
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Eye,
  KeyRound,
  Pencil,
  Plus,
  Search,
  Table2,
  Trash2,
  X
} from "lucide-react"
import { useTranslation } from "react-i18next"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Field as FormField, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { SqliteLocationFields } from "./SqliteLocationFields"
import { DatabaseTableActions } from "@/app/panels/DatabaseTableActions"
import { useHostStore } from "@/state/hostStore"
import { ScrollArea } from "@/components/ui/scroll-area"
import { dbObjectRefKey } from "@/lib/databaseSql"
import { dbPostgresTransportChallenge, dbTestConnection } from "@/lib/ipc"
import { relativeTime } from "@/lib/relativeTime"
import type {
  DbDescriptorId,
  DbKind,
  DbOpenConfig,
  DbPostgresTransportChallenge,
  DbProfileTarget,
  DbTable,
  PostgresTransportMode
} from "@/lib/types"
import {
  DEFAULT_POSTGRES_TRANSPORT_MODE,
  migrateLegacyPostgresTransport,
  postgresInsecureExceptionMatches,
  postgresTransportAcknowledged,
  postgresTransportIdentityMatches
} from "@/lib/types"
import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import {
  dbProfileNeedsCredentialPrompt,
  dbProfileUiErrorCode,
  queryFor,
  savedConnectionAddress,
  useDbStore,
  type DbProfileUiErrorCode,
  type DbSessionStatus,
  type SavedDbConnection
} from "@/state/dbStore"

/** The dialog opens with one of three intents: a brand-new connection, a
 *  password-only reconnect (connection fields locked), or a full edit of an
 *  existing descriptor (all fields editable). */
type DialogMode = "new" | "reconnect" | "edit"
type EditCredentialAction = "keep" | "replace" | "remove"
type RecoveryRunOutcome = "completed" | "credentialPrompt" | "failed"

/**
 * Database mode nav content (FEAT-1 + F2). Lists persisted connection
 * descriptors — SQLite files and network (PostgreSQL/MSSQL) endpoints — each
 * either connected (active highlighted, closable) or saved-but-offline (click to
 * reconnect). The active connection's tables/views and recent queries hang below.
 * The bottom action opens the "New connection" dialog. Credentials are written
 * to the OS vault and never persisted in React/Zustand/localStorage state.
 */
export function DatabaseNavContent() {
  const { t } = useTranslation("workbench")
  const recovery = useDbStore((s) => s.recovery)
  const profileError = useDbStore((s) => s.profileError)
  const initializeProfiles = useDbStore((s) => s.initializeProfiles)
  const reconnectRequest = useDbStore((s) => s.reconnectRequest)
  const consumeReconnectRequest = useDbStore((s) => s.consumeReconnectRequest)
  const recoverProfile = useDbStore((s) => s.recoverProfile)
  const savedCount = useDbStore((s) => s.saved.length)

  const [dialogOpen, setDialogOpen] = useState(false)
  const [dialogMode, setDialogMode] = useState<DialogMode>("new")
  const [dialogInstance, setDialogInstance] = useState(0)
  const latestDialogInstance = useRef(0)
  const lastHandledReconnectToken = useRef<number | null>(null)
  // Prefill for reconnect / edit; null for a brand-new connection.
  const [prefill, setPrefill] = useState<SavedDbConnection | null>(null)
  const [actionError, setActionError] = useState<DbProfileUiErrorCode | null>(null)
  const [recoveryOperationId, setRecoveryOperationId] = useState<string | null>(null)
  const [recoveryPassword, setRecoveryPassword] = useState("")
  const [recoveryBusy, setRecoveryBusy] = useState(false)
  const recoveryInFlight = useRef(false)

  const visibleProfileError = profileError
    ? t(`database.profileError.${profileError}`)
    : null

  useEffect(() => {
    void initializeProfiles()
  }, [initializeProfiles])

  async function runRecovery(
    operationId: string,
    action: "resume" | "abort" | "retryCleanup",
    password?: string
  ): Promise<RecoveryRunOutcome> {
    setActionError(null)
    try {
      await recoverProfile({
        operationId,
        action,
        credential: password ? { password } : null
      })
      return "completed"
    } catch (error) {
      const code = dbProfileUiErrorCode(error)
      if (action === "resume" && password === undefined && dbProfileNeedsCredentialPrompt(code)) {
        return "credentialPrompt"
      }
      setActionError(code)
      return "failed"
    }
  }

  async function executeRecoveryAction(
    operationId: string,
    action: "resume" | "abort" | "retryCleanup",
    password?: string
  ): Promise<RecoveryRunOutcome> {
    if (recoveryInFlight.current) return "failed"
    recoveryInFlight.current = true
    setRecoveryBusy(true)
    try {
      return await runRecovery(operationId, action, password)
    } finally {
      recoveryInFlight.current = false
      setRecoveryBusy(false)
    }
  }

  function beginDialogInstance() {
    const next = latestDialogInstance.current + 1
    latestDialogInstance.current = next
    setDialogInstance(next)
    setDialogOpen(true)
  }

  function openNew() {
    setPrefill(null)
    setDialogMode("new")
    beginDialogInstance()
  }

  function openEdit(entry: SavedDbConnection) {
    setPrefill(entry)
    setDialogMode("edit")
    beginDialogInstance()
  }

  useEffect(() => {
    if (!reconnectRequest) return
    const request = reconnectRequest
    let cancelled = false
    queueMicrotask(() => {
      if (cancelled) return
      if (lastHandledReconnectToken.current === request.token) return
      const current = useDbStore.getState().reconnectRequest
      // Recheck after the effect boundary so an obsolete request cannot open a
      // dialog after a newer request or another surface consumed it.
      if (current?.token !== request.token) return
      lastHandledReconnectToken.current = request.token
      const entry = useDbStore.getState().saved.find(
        (candidate) => candidate.id === request.descriptorId
      )
      consumeReconnectRequest(request.token)
      // The shared command only requests network reconnects. Recheck at the UI
      // boundary so a removed/edited descriptor cannot open a stale dialog.
      if (!entry || entry.kind === "sqlite") return
      setPrefill(entry)
      setDialogMode("reconnect")
      beginDialogInstance()
    })
    return () => {
      cancelled = true
    }
  }, [consumeReconnectRequest, reconnectRequest])

  return (
    <div data-testid="db-nav-root" className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="database-nav-header flex h-11 shrink-0 items-center gap-2 border-b border-(--line-1) pr-3">
        <h2 id="db-saved-heading" className="min-w-0 truncate text-[13px] font-medium text-(--ink-1)">
          {t("database.savedConnectionsHeading")}
        </h2>
        {savedCount > 0 && (
          <span aria-hidden="true" className="shrink-0 font-mono text-[11px] text-(--ink-4) tabular-nums">{savedCount}</span>
        )}
      </div>
      {(visibleProfileError || actionError) && (
        <p role="alert" className="mx-2 mt-2 shrink-0 rounded-(--r-xs) bg-(--danger-soft) px-2.5 py-1.5 text-[12px] text-(--destructive)">
          {actionError ? t(`database.profileError.${actionError}`) : visibleProfileError}
        </p>
      )}
      {recovery.length > 0 && (
        <ScrollArea className="mx-2 mt-2 max-h-[112px] shrink-0 rounded-(--r-xs) bg-(--amber-soft)" viewportClassName="px-2.5 py-2">
        <section aria-label={t("database.recoveryHeading")} className="flex flex-col gap-1.5">
          <span className="flex items-center gap-1.5 text-[12px] font-medium text-(--ink-1)">
            <AlertTriangle className="size-3.5" aria-hidden="true" />
            {t("database.recoveryHeading")}
          </span>
          {recovery.map((row) => (
            <div key={row.operationId} className="flex flex-wrap items-center gap-1 text-[11.5px] text-(--ink-2)">
              <span className="min-w-0 flex-1 basis-full truncate font-mono" title={row.descriptorId}>
                {t(`database.recoveryKind.${row.kind}`)} {row.descriptorId}
              </span>
              {row.allowedActions.includes("resume") && (
                <Button
                  size="xs"
                  variant="outline"
                  disabled={recoveryBusy}
                  onClick={() => void executeRecoveryAction(row.operationId, "resume").then((outcome) => {
                    if (outcome === "credentialPrompt") {
                      setRecoveryOperationId(row.operationId)
                      setRecoveryPassword("")
                    }
                  })}
                >
                  {t("database.recoveryResume")}
                </Button>
              )}
              {row.allowedActions.includes("abort") && (
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={recoveryBusy}
                  onClick={() => void executeRecoveryAction(row.operationId, "abort")}
                >
                  {t("database.recoveryAbort")}
                </Button>
              )}
              {row.allowedActions.includes("retryCleanup") && (
                <Button
                  size="xs"
                  variant="outline"
                  disabled={recoveryBusy}
                  onClick={() => void executeRecoveryAction(row.operationId, "retryCleanup")}
                >
                  {t("database.recoveryRetry")}
                </Button>
              )}
            </div>
          ))}
        </section>
        </ScrollArea>
      )}
      <div
        data-testid="db-region-grid"
        className="grid min-h-0 flex-1 grid-rows-[fit-content(40%)_minmax(96px,1fr)_auto] overflow-hidden"
      >
        <SavedConnectionsRegion onOpenEdit={openEdit} onError={setActionError} />
        <DatabaseObjectTreeRegion />
        <RecentQueriesRegion />
      </div>

      <div data-testid="db-new-connection" className="shrink-0 border-t border-(--line-1) p-1.5">
        <Button
          type="button"
          variant="ghost"
          onClick={openNew}
          className="h-8 w-full justify-start gap-2 px-2.5 text-[12.5px] font-normal text-(--ink-2) hover:bg-(--db-hover) hover:text-(--ink-1)"
        >
          <Plus aria-hidden="true" />
          {t("database.newConnection")}
        </Button>
      </div>

      <Dialog
        open={recoveryOperationId !== null}
        onOpenChange={(open) => {
          if (!open && !recoveryBusy) {
            setRecoveryOperationId(null)
            setRecoveryPassword("")
          }
        }}
      >
        <DialogContent
          className="flex max-h-[calc(100vh-2rem)] min-h-0 flex-col gap-0 overflow-hidden p-0"
          data-testid="database-recovery-dialog"
        >
          <DialogHeader className="shrink-0 px-4 pt-4">
            <DialogTitle>{t("database.recoveryCredentialTitle")}</DialogTitle>
            <DialogDescription>{t("database.recoveryCredentialDescription")}</DialogDescription>
          </DialogHeader>
          <ScrollArea
            data-testid="database-recovery-body"
            className="min-h-0 flex-1"
            viewportClassName="px-4 py-3"
            contentClassName="flex flex-col gap-[10px]"
          >
            <Input
              type="password"
              autoComplete="new-password"
              value={recoveryPassword}
              onChange={(event) => setRecoveryPassword(event.target.value)}
              aria-label={t("database.fieldPassword")}
            />
          </ScrollArea>
          <DialogFooter className="mx-0 mb-0 shrink-0 rounded-none border-t px-4 py-4">
            <Button variant="outline" disabled={recoveryBusy} onClick={() => setRecoveryOperationId(null)}>
              {t("database.cancel")}
            </Button>
            <Button
              disabled={recoveryBusy || recoveryPassword.length === 0}
              onClick={() => {
                if (!recoveryOperationId) return
                void executeRecoveryAction(recoveryOperationId, "resume", recoveryPassword).then((outcome) => {
                  if (outcome === "completed") {
                    setRecoveryOperationId(null)
                    setRecoveryPassword("")
                  }
                })
              }}
            >
              {t("database.recoveryResume")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <NewConnectionDialog
        key={`${dialogInstance}:${dialogMode}:${prefill?.id ?? "new"}`}
        open={dialogOpen}
        mode={dialogMode}
        onOpenChange={(next) => {
          // Ignore completion from a dismissed async submit after another dialog
          // instance has opened; it must not close the user's newer form.
          if (dialogInstance !== latestDialogInstance.current) return
          setDialogOpen(next)
          if (!next) {
            setDialogMode("new")
            setPrefill(null)
          }
        }}
        prefill={prefill}
      />
    </div>
  )
}

const rowActionClass = "flex size-6 items-center justify-center rounded-[5px] text-(--ink-3) transition-colors hover:bg-(--db-hover) hover:text-(--ink-1) focus-visible:outline-2 focus-visible:outline-(--ring)"

function SavedConnectionsRegion({
  onOpenEdit,
  onError
}: {
  onOpenEdit: (entry: SavedDbConnection) => void
  onError: (error: DbProfileUiErrorCode) => void
}) {
  const { t } = useTranslation("workbench")
  const connections = useDbStore((s) => s.connections)
  const activeDescriptorId = useDbStore((s) => s.activeDescriptorId)
  const saved = useDbStore((s) => s.saved)
  const sessions = useDbStore((s) => s.sessions)
  const openOrReconnectSavedConnection = useDbStore((s) => s.openOrReconnectSavedConnection)
  const removeSaved = useDbStore((s) => s.removeSaved)
  const removeCredential = useDbStore((s) => s.removeCredential)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const liveByDescriptorId = new Map(
    connections.map((connection) => [connection.descriptorId, connection])
  )

  return (
    <section
      data-testid="db-saved-region"
      aria-labelledby="db-saved-heading"
      className="flex min-h-0 flex-col overflow-hidden"
    >
      <ScrollArea
        data-testid="db-saved-scroll"
        className="min-h-0 flex-1"
        viewportClassName="[&>div]:block!"
      >
        {saved.length === 0 ? (
          <div className="px-4 py-5">
            <p className="text-[12.5px] font-medium text-(--ink-2)">{t("database.emptyTitle")}</p>
            <p className="mt-1 text-[12px] text-(--ink-3)">{t("database.emptyDescription")}</p>
          </div>
        ) : (
          <ul className="flex flex-col gap-px p-1.5">
            {saved.map((entry) => {
              const live = liveByDescriptorId.get(entry.id)
              const isActive = entry.id === activeDescriptorId
              const session = sessions[entry.id]
              const status: DbSessionStatus = session?.status ?? (live ? "connected" : "disconnected")
              const semanticStatus = status === "disconnected" ? "offline" : status
              const semanticCredentialState = entry.credentialState === "required"
                ? "credentialRequired"
                : entry.credentialState === "unavailable"
                  ? "vaultUnavailable"
                  : (entry.credentialState ?? "notRequired")
              const credentialNote = entry.credentialState === "required"
                ? t("database.credentialRequired")
                : entry.credentialState === "unavailable"
                  ? t("database.vaultUnavailable")
                  : null
              return (
                <li
                  key={entry.id}
                  data-testid="db-saved-row"
                  data-descriptor-id={entry.id}
                  data-status={semanticStatus}
                  data-credential-state={semanticCredentialState}
                  data-active={isActive ? "true" : "false"}
                >
                  <div
                    onContextMenu={contextMenuHandler({
                      kind: "dbconn",
                      descriptorId: entry.id,
                      address: savedConnectionAddress(entry)
                    })}
                    className={cn(
                      "group relative flex h-10 items-center rounded-(--r-xs) transition-colors",
                      isActive ? "bg-(--db-selected)" : "hover:bg-(--db-hover)"
                    )}
                  >
                    <button
                      type="button"
                      aria-current={isActive ? "true" : undefined}
                      onClick={() => void openOrReconnectSavedConnection(entry.id)}
                      title={savedConnectionAddress(entry)}
                      className="flex h-full min-w-0 flex-1 items-center gap-2.5 rounded-(--r-xs) px-2.5 text-left outline-none focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
                    >
                      <StatusDot status={status} />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className={cn(
                          "truncate text-[13px] leading-[18px]",
                          isActive ? "font-medium text-(--ink-1)" : live ? "text-(--ink-1)" : "text-(--ink-2)"
                        )}>
                          {entry.name}
                        </span>
                        <span className="flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-(--ink-3)">
                          <span className="truncate">{kindLabel(entry.kind)}{credentialNote ? ` · ${credentialNote}` : ""}</span>
                          {!live && <span className="sr-only">{t("database.savedOffline")}</span>}
                        </span>
                      </span>
                      <StatusBadge status={status} error={session?.error ?? null} />
                    </button>
                    {confirmDeleteId === entry.id ? (
                      <div className="absolute right-1.5 flex items-center gap-0.5 rounded-(--r-xs) border border-(--line-1) bg-(--paper-0) p-0.5 shadow-xs">
                        <button
                          type="button"
                          aria-label={t("database.confirmRemove", { name: entry.name })}
                          onClick={() => {
                            setConfirmDeleteId(null)
                            void removeSaved(entry.id).catch((error) => {
                              onError(dbProfileUiErrorCode(error))
                            })
                          }}
                          className={cn(rowActionClass, "text-(--destructive) hover:bg-(--danger-soft) hover:text-(--destructive)")}
                        >
                          <Check className="size-3.5" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          aria-label={t("database.cancelRemove")}
                          onClick={() => setConfirmDeleteId(null)}
                          className={rowActionClass}
                        >
                          <X className="size-3.5" aria-hidden="true" />
                        </button>
                      </div>
                    ) : (
                      <div className="absolute right-1.5 flex items-center gap-0.5 rounded-(--r-xs) border border-(--line-1) bg-(--paper-0) p-0.5 opacity-0 shadow-xs transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 motion-reduce:transition-none">
                        {entry.kind !== "sqlite" && entry.credentialState === "stored" && (
                          <button
                            type="button"
                            aria-label={t("database.removeCredential", { name: entry.name })}
                            title={t("database.removeCredential", { name: entry.name })}
                            onClick={() => void removeCredential(entry.id).catch((error) => {
                              onError(dbProfileUiErrorCode(error))
                            })}
                            className={rowActionClass}
                          >
                            <KeyRound className="size-3.5" aria-hidden="true" />
                          </button>
                        )}
                        <button
                          type="button"
                          aria-label={t("database.editConnection", { name: entry.name })}
                          title={t("database.editConnection", { name: entry.name })}
                          onClick={() => onOpenEdit(entry)}
                          className={rowActionClass}
                        >
                          <Pencil className="size-3.5" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          aria-label={t("database.forgetConnection", { name: entry.name })}
                          title={t("database.forgetConnection", { name: entry.name })}
                          onClick={() => setConfirmDeleteId(entry.id)}
                          className={rowActionClass}
                        >
                          <Trash2 className="size-3.5" aria-hidden="true" />
                        </button>
                      </div>
                    )}
                  </div>
                </li>
              )
            })}
          </ul>
        )}
      </ScrollArea>
    </section>
  )
}

interface DbObjectSchemaGroup {
  schema: string
  tables: DbTable[]
  views: DbTable[]
}

interface DbObjectCatalogGroup {
  catalog: string
  schemas: DbObjectSchemaGroup[]
}

function groupDatabaseObjects(objects: DbTable[]): DbObjectCatalogGroup[] {
  const catalogs = new Map<string, Map<string, { tables: DbTable[]; views: DbTable[] }>>()
  for (const object of objects) {
    let schemas = catalogs.get(object.catalog)
    if (!schemas) {
      schemas = new Map()
      catalogs.set(object.catalog, schemas)
    }
    let kinds = schemas.get(object.schema)
    if (!kinds) {
      kinds = { tables: [], views: [] }
      schemas.set(object.schema, kinds)
    }
    kinds[object.kind === "table" ? "tables" : "views"].push(object)
  }
  return [...catalogs.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([catalog, schemas]) => ({
      catalog,
      schemas: [...schemas.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([schema, kinds]) => ({
          schema,
          tables: kinds.tables.sort((left, right) => left.name.localeCompare(right.name)),
          views: kinds.views.sort((left, right) => left.name.localeCompare(right.name))
        }))
    }))
}

/** Case-insensitive name filter for the object tree. An empty query keeps all. */
function filterDatabaseObjects(objects: DbTable[], query: string): DbTable[] {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return objects
  return objects.filter((object) => object.name.toLocaleLowerCase().includes(needle))
}

function DatabaseObjectTreeRegion() {
  const { t } = useTranslation("workbench")
  const connections = useDbStore((s) => s.connections)
  const activeDescriptorId = useDbStore((s) => s.activeDescriptorId)
  const activeConnId = useDbStore((s) => s.activeConnId)
  const tableBuckets = useDbStore((s) => s.tableBuckets)
  const tableErrors = useDbStore((s) => s.tableErrors)
  const columnBuckets = useDbStore((s) => s.columnBuckets)
  const columnErrors = useDbStore((s) => s.columnErrors)
  const tables = useDbStore((s) => s.tables)
  const loadTables = useDbStore((s) => s.loadTables)
  const loadColumns = useDbStore((s) => s.loadColumns)
  const openTableQuery = useDbStore((s) => s.openTableQuery)
  const activeTableKey = useDbStore((s) => {
    const table = s.activeDescriptorId ? queryFor(s, s.activeDescriptorId).table : null
    return table ? dbObjectRefKey(table) : null
  })
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set())
  const [expandedObjects, setExpandedObjects] = useState<Set<string>>(() => new Set())
  const [filter, setFilter] = useState("")

  const activeConn = connections.find((connection) =>
    connection.descriptorId === activeDescriptorId
  )
  const activeTables = activeDescriptorId
    ? (tableBuckets[activeDescriptorId] ?? (activeConnId ? tables[activeConnId] : undefined) ?? [])
    : []
  const activeTableError = activeDescriptorId ? tableErrors[activeDescriptorId] ?? null : null
  const filtering = filter.trim().length > 0
  const visibleTables = filterDatabaseObjects(activeTables, filter)
  const groups = groupDatabaseObjects(visibleTables)
  // While filtering, every matching branch stays open so matches are never hidden.
  const isExpanded = (key: string) => filtering || !collapsedGroups.has(key)

  function toggleGroup(key: string) {
    setCollapsedGroups((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  function toggleObject(object: DbTable) {
    if (!activeDescriptorId) return
    const refKey = dbObjectRefKey(object)
    const expansionKey = `${activeDescriptorId}:${refKey}`
    const objectExpanded = expandedObjects.has(expansionKey)
    setExpandedObjects((current) => {
      const next = new Set(current)
      if (objectExpanded) next.delete(expansionKey)
      else next.add(expansionKey)
      return next
    })
    if (
      !objectExpanded
      && columnBuckets[activeDescriptorId]?.[refKey] === undefined
      && !columnErrors[activeDescriptorId]?.[refKey]
    ) {
      void loadColumns(activeDescriptorId, object)
    }
  }

  const connected = !!activeConn && !!activeDescriptorId

  return (
    <section
      data-testid="db-object-region"
      aria-labelledby="db-object-heading"
      className="flex min-h-0 flex-col overflow-hidden border-t border-(--line-1)"
    >
      <h2 id="db-object-heading" className="sr-only">{t("database.objectTreeHeading")}</h2>
      <div className="shrink-0 px-2 pt-2 pb-1.5">
        <InputGroup className="h-7 rounded-(--r-xs) border-(--line-1) bg-(--paper-0) shadow-none">
          <InputGroupAddon>
            <Search className="size-3.5" aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            type="search"
            value={filter}
            disabled={!connected || activeTables.length === 0}
            onChange={(event) => setFilter(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && filter) {
                event.preventDefault()
                setFilter("")
              }
            }}
            placeholder={t("database.filterObjectsPlaceholder")}
            aria-label={t("database.filterObjects")}
            className="text-[12.5px] [&::-webkit-search-cancel-button]:hidden"
          />
          {activeTables.length > 0 && (
            <InputGroupAddon align="inline-end">
              <span aria-hidden="true" className="font-mono text-[11px] text-(--ink-4) tabular-nums">
                {filtering ? `${visibleTables.length}/${activeTables.length}` : activeTables.length}
              </span>
            </InputGroupAddon>
          )}
        </InputGroup>
      </div>
      <ScrollArea
        data-testid="db-object-scroll"
        className="min-h-0 flex-1"
        viewportClassName="px-1.5 pb-2 [&>div]:block!"
      >
        {activeTableError && activeDescriptorId && (
          <div
            role="alert"
            className="mb-1.5 flex items-start gap-2 rounded-(--r-xs) bg-(--amber-soft) px-2.5 py-2 text-[12px] text-(--ink-2)"
          >
            <span className="min-w-0 flex-1">
              {activeTableError.code === "connectionBusy"
                ? t("database.tableConnectionBusy")
                : t("database.tableRefreshFailed")}
            </span>
            <Button
              size="xs"
              variant="outline"
              aria-label={t("database.retryObjectRefresh")}
              onClick={() => void loadTables(activeDescriptorId)}
            >
              {t("database.retry")}
            </Button>
          </div>
        )}
        {!connected ? (
          <p className="px-2.5 py-2 text-[12px] text-(--ink-3)">
            {t("database.noActiveConnectionObjects")}
          </p>
        ) : activeTables.length === 0 ? (
          <p className="px-2.5 py-2 text-[12px] text-(--ink-3)">{t("database.noTables")}</p>
        ) : groups.length === 0 ? (
          <p role="status" className="px-2.5 py-2 text-[12px] text-(--ink-3)">
            {t("database.noObjectMatches", { query: filter.trim() })}
          </p>
        ) : (
          <ul className="flex flex-col">
            {groups.map((catalogGroup) => {
              const catalogKey = JSON.stringify([activeDescriptorId, "catalog", catalogGroup.catalog])
              const catalogExpanded = isExpanded(catalogKey)
              return (
                <li key={catalogKey}>
                  <TreeGroupToggle
                    expanded={catalogExpanded}
                    label={t("database.catalogGroup", { name: catalogGroup.catalog })}
                    kindLabel={t("database.catalogShort")}
                    name={catalogGroup.catalog}
                    level="catalog"
                    groupKey={catalogKey}
                    onClick={() => toggleGroup(catalogKey)}
                  />
                  {catalogExpanded && (
                    <ul className="ml-[13px] border-l border-(--line-1) pl-1">
                      {catalogGroup.schemas.map((schemaGroup) => {
                        const schemaKey = JSON.stringify([
                          activeDescriptorId,
                          "schema",
                          catalogGroup.catalog,
                          schemaGroup.schema
                        ])
                        const schemaExpanded = isExpanded(schemaKey)
                        return (
                          <li key={schemaKey}>
                            <TreeGroupToggle
                              expanded={schemaExpanded}
                              label={t("database.schemaGroup", { name: schemaGroup.schema })}
                              kindLabel={t("database.schemaShort")}
                              name={schemaGroup.schema}
                              level="schema"
                              groupKey={schemaKey}
                              onClick={() => toggleGroup(schemaKey)}
                            />
                            {schemaExpanded && (
                              <ul className="ml-[13px] border-l border-(--line-1) pl-1">
                                {(["table", "view"] as const).map((kind) => {
                                  const objects = kind === "table" ? schemaGroup.tables : schemaGroup.views
                                  if (objects.length === 0) return null
                                  const kindKey = JSON.stringify([
                                    activeDescriptorId,
                                    "kind",
                                    catalogGroup.catalog,
                                    schemaGroup.schema,
                                    kind
                                  ])
                                  const kindExpanded = isExpanded(kindKey)
                                  return (
                                    <li key={kindKey}>
                                      <TreeGroupToggle
                                        expanded={kindExpanded}
                                        label={kind === "table"
                                          ? t("database.tableGroupHeading")
                                          : t("database.viewGroupHeading")}
                                        level="kind"
                                        count={objects.length}
                                        groupKey={kindKey}
                                        onClick={() => toggleGroup(kindKey)}
                                      />
                                      {kindExpanded && (
                                        <ul className="flex flex-col">
                                          {objects.map((object) => {
                                            const refKey = dbObjectRefKey(object)
                                            const expansionKey = `${activeDescriptorId}:${refKey}`
                                            const objectExpanded = expandedObjects.has(expansionKey)
                                            const columns = columnBuckets[activeDescriptorId]?.[refKey]
                                            const columnError = columnErrors[activeDescriptorId]?.[refKey] ?? null
                                            const Icon = object.kind === "view" ? Eye : Table2
                                            const isActive = activeTableKey === refKey
                                            return (
                                              <li
                                                key={refKey}
                                                data-testid="db-object-row"
                                                data-object-ref={refKey}
                                              >
                                                <DatabaseTableActions descriptorId={activeDescriptorId} table={object}>
                                                <div
                                                  data-active={isActive || undefined}
                                                  className="group/object flex h-7 items-center rounded-(--r-xs) text-(--ink-2) transition-colors hover:bg-(--db-hover) hover:text-(--ink-1) data-active:bg-(--db-selected) data-active:text-(--ink-1)"
                                                >
                                                  <button
                                                    type="button"
                                                    aria-label={objectExpanded
                                                      ? t("database.collapseColumns", { name: object.name })
                                                      : t("database.expandColumns", { name: object.name })}
                                                    aria-expanded={objectExpanded}
                                                    onClick={() => toggleObject(object)}
                                                    className="flex h-full w-5 shrink-0 items-center justify-center rounded-(--r-xs) text-(--ink-4) outline-none hover:text-(--ink-1) focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
                                                  >
                                                    <ChevronRight
                                                      className={cn("size-3 transition-transform motion-reduce:transition-none", objectExpanded && "rotate-90")}
                                                      aria-hidden="true"
                                                    />
                                                  </button>
                                                  <button
                                                    type="button"
                                                    onClick={() => void openTableQuery(object)}
                                                    title={`${object.catalog}.${object.schema}.${object.name}`}
                                                    className="flex h-full min-w-0 flex-1 items-center gap-2 rounded-(--r-xs) pr-2 text-left outline-none focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
                                                  >
                                                    <Icon
                                                      aria-hidden="true"
                                                      className={cn("size-3.5 shrink-0", isActive ? "text-(--yz-accent-ink)" : "text-(--ink-4) group-hover/object:text-(--ink-3)")}
                                                    />
                                                    <span className="truncate font-mono text-[12px]">{object.name}</span>
                                                  </button>
                                                </div>
                                                </DatabaseTableActions>
                                                {objectExpanded && (
                                                  <div className="mb-1 ml-[9px] border-l border-(--line-1) pl-3">
                                                    {columnError ? (
                                                      <div role="alert" className="flex items-center gap-1.5 py-1 text-[11.5px] text-(--ink-3)">
                                                        <span className="min-w-0 flex-1">
                                                          {columnError.code === "connectionBusy"
                                                            ? t("database.tableConnectionBusy")
                                                            : t("database.columnRefreshFailed")}
                                                        </span>
                                                        <Button
                                                          type="button"
                                                          size="xs"
                                                          variant="ghost"
                                                          aria-label={t("database.retryColumns", { name: object.name })}
                                                          onClick={() => void loadColumns(activeDescriptorId, object)}
                                                          className="text-(--yz-accent-ink)"
                                                        >
                                                          {t("database.retry")}
                                                        </Button>
                                                      </div>
                                                    ) : columns === undefined ? (
                                                      <p role="status" className="py-1 text-[11.5px] text-(--ink-3)">
                                                        {t("database.loadingColumns")}
                                                      </p>
                                                    ) : columns.length === 0 ? (
                                                      <p className="py-1 text-[11.5px] text-(--ink-3)">
                                                        {t("database.noColumns")}
                                                      </p>
                                                    ) : (
                                                      <ul className="py-0.5">
                                                        {columns.map((column) => (
                                                          <li
                                                            key={column.name}
                                                            title={`${column.name} ${column.type}`}
                                                            className="flex h-[22px] items-center gap-1.5 font-mono text-[11px] text-(--ink-3)"
                                                          >
                                                            <span className="min-w-0 truncate text-(--ink-2)">
                                                              {column.name}
                                                            </span>
                                                            {column.pk && (
                                                              <span className="shrink-0 text-[10px] font-medium text-(--yz-accent-ink)">
                                                                {t("database.columnPrimaryKey")}
                                                              </span>
                                                            )}
                                                            <span className="ml-auto max-w-[96px] shrink-0 truncate text-(--ink-4)">{column.type}</span>
                                                            <span className={cn("shrink-0 text-[10px]", column.notnull ? "text-(--ink-3)" : "text-(--ink-4)")}>
                                                              {column.notnull
                                                                ? t("database.columnNotNull")
                                                                : t("database.columnNullable")}
                                                            </span>
                                                          </li>
                                                        ))}
                                                      </ul>
                                                    )}
                                                  </div>
                                                )}
                                              </li>
                                            )
                                          })}
                                        </ul>
                                      )}
                                    </li>
                                  )
                                })}
                              </ul>
                            )}
                          </li>
                        )
                      })}
                    </ul>
                  )}
                </li>
              )
            })}
          </ul>
        )}
      </ScrollArea>
    </section>
  )
}

function TreeGroupToggle({
  expanded,
  label,
  kindLabel,
  name,
  level,
  groupKey,
  count,
  onClick
}: {
  expanded: boolean
  label: string
  kindLabel?: string
  name?: string
  level: "catalog" | "schema" | "kind"
  groupKey: string
  count?: number
  onClick: () => void
}) {
  return (
    <button
      type="button"
      data-testid="db-object-group-toggle"
      data-group-level={level}
      data-group-key={groupKey}
      aria-expanded={expanded}
      aria-label={name !== undefined ? label : undefined}
      onClick={onClick}
      className="flex h-7 w-full items-center gap-1.5 rounded-(--r-xs) pr-2 pl-1 text-left text-[12px] text-(--ink-2) outline-none transition-colors hover:bg-(--db-hover) hover:text-(--ink-1) focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
    >
      <ChevronRight
        className={cn("size-3 shrink-0 text-(--ink-4) transition-transform motion-reduce:transition-none", expanded && "rotate-90")}
        aria-hidden="true"
      />
      {name !== undefined ? (
        <span className="flex min-w-0 items-baseline gap-1.5" aria-hidden="true">
          <span className="shrink-0 text-[11px] text-(--ink-4)">{kindLabel}</span>
          <span className="truncate font-mono text-[12px] text-(--ink-1)">{name}</span>
        </span>
      ) : (
        <span className="truncate font-medium text-(--ink-3)">{label}</span>
      )}
      {count !== undefined && (
        <span aria-hidden="true" className="ml-auto shrink-0 font-mono text-[11px] text-(--ink-4) tabular-nums">{count}</span>
      )}
    </button>
  )
}

function RecentQueriesRegion() {
  const { t } = useTranslation("workbench")
  const activeDescriptorId = useDbStore((s) => s.activeDescriptorId)
  const historyBuckets = useDbStore((s) => s.historyBuckets)
  const setSql = useDbStore((s) => s.setSql)
  const [expanded, setExpanded] = useState(true)
  const historyEntries = activeDescriptorId
    ? (historyBuckets[activeDescriptorId] ?? [])
    : []

  return (
    <section
      data-testid="db-history-region"
      aria-labelledby="db-history-heading"
      className="flex max-h-[200px] min-h-0 flex-col overflow-hidden border-t border-(--line-1)"
    >
      <button
        id="db-history-heading"
        type="button"
        data-testid="db-history-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
        className="flex h-9 shrink-0 items-center gap-1.5 px-2.5 text-left text-[12px] font-medium text-(--ink-2) outline-none transition-colors hover:bg-(--db-hover) hover:text-(--ink-1) focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
      >
        <ChevronRight
          className={cn("size-3 shrink-0 text-(--ink-4) transition-transform motion-reduce:transition-none", expanded && "rotate-90")}
          aria-hidden="true"
        />
        <span className="min-w-0 flex-1 truncate">{t("database.recentQueriesHeading")}</span>
        {historyEntries.length > 0 && (
          <span className="font-mono text-[11px] font-normal text-(--ink-4) tabular-nums" aria-hidden="true">{historyEntries.length}</span>
        )}
      </button>
      {expanded && (
        <ScrollArea
          data-testid="db-history-scroll"
          className="min-h-0 max-h-[164px]"
          viewportClassName="px-1.5 pb-1.5 [&>div]:block!"
        >
          {historyEntries.length === 0 ? (
            <p className="px-2.5 pb-1.5 text-[12px] text-(--ink-3)">
              {t("database.noRecentQueries")}
            </p>
          ) : (
            <ul className="flex flex-col">
              {historyEntries.map((entry, index) => (
                <li
                  key={`${entry.ranAt}-${index}`}
                  data-testid="db-history-row"
                >
                  <button
                    type="button"
                    onClick={() => setSql(entry.sql)}
                    title={entry.sql}
                    className="flex h-7 w-full items-center gap-2 rounded-(--r-xs) px-2.5 text-left text-(--ink-2) outline-none transition-colors hover:bg-(--db-hover) hover:text-(--ink-1) focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
                  >
                    <span className="flex size-2 shrink-0 items-center justify-center">
                      {!entry.ok && (
                        <span
                          role="img"
                          aria-label={t("database.historyFailed")}
                          title={entry.error}
                          className="size-1.5 rounded-full bg-(--destructive)"
                        />
                      )}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-[12px]">
                      {entry.sql.split("\n")[0]}
                    </span>
                    <span className="shrink-0 font-mono text-[10.5px] text-(--ink-4) tabular-nums">
                      {relativeTime(Math.floor(entry.ranAt / 1000))}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </ScrollArea>
      )}
    </section>
  )
}

function StatusDot({ status }: { status: DbSessionStatus }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "size-2 shrink-0 rounded-full",
        status === "connected" && "bg-(--term-ok)",
        status === "connecting" && "bg-(--term-amber) motion-safe:animate-pulse",
        status === "error" && "bg-(--destructive)",
        status === "disconnected" && "border border-(--ink-4)"
      )}
    />
  )
}

function StatusBadge({
  status,
  error
}: {
  status: DbSessionStatus
  error: DbProfileUiErrorCode | null
}) {
  const { t } = useTranslation("workbench")
  const map: Record<DbSessionStatus, { label: string; className: string }> = {
    connecting: { label: t("database.statusConnecting"), className: "text-(--ink-2)" },
    // Connected/offline are already carried by the status dot and row tone;
    // keep the word for assistive tech without repeating it visually.
    connected: { label: t("database.statusConnected"), className: "sr-only" },
    error: { label: t("database.statusError"), className: "font-medium text-(--destructive)" },
    disconnected: { label: t("database.statusOffline"), className: "sr-only" }
  }
  const { label, className } = map[status]
  return (
    <span
      data-status={status === "disconnected" ? "offline" : status}
      title={status === "error" && error ? t(`database.profileError.${error}`) : undefined}
      className={cn("shrink-0 text-[11px]", className)}
    >
      {label}
    </span>
  )
}

function kindLabel(kind: DbKind): string {
  return kind === "sqlite" ? "SQLite" : kind === "postgres" ? "PostgreSQL" : "MSSQL"
}

function defaultPort(kind: DbKind): number {
  return kind === "postgres" ? 5432 : 1433
}

function savedProfileTarget(entry: SavedDbConnection): DbProfileTarget | null {
  if (entry.kind === "sqlite") {
    return entry.path ? { kind: "sqlite", path: entry.path, ...(entry.workspace ? { workspace: entry.workspace } : {}) } : null
  }
  if (!entry.host || !entry.port || typeof entry.database !== "string" || !entry.user) return null
  if (entry.kind === "postgres") {
    return {
      kind: "postgres",
      host: entry.host,
      ...(entry.viaHost ? { viaHost: entry.viaHost } : {}),
      port: entry.port,
      database: entry.database,
      user: entry.user,
      ...migrateLegacyPostgresTransport(entry)
    }
  }
  return {
    kind: "mssql",
    host: entry.host,
      ...(entry.viaHost ? { viaHost: entry.viaHost } : {}),
    port: entry.port,
    database: entry.database,
    user: entry.user,
    trustCert: entry.trustCert ?? false
  }
}

function sameProfileTarget(left: DbProfileTarget, right: DbProfileTarget): boolean {
  if (left.kind !== right.kind) return false
  if (left.kind === "sqlite" && right.kind === "sqlite") return left.path === right.path && left.workspace?.hostId === right.workspace?.hostId && left.workspace?.canonicalPath === right.workspace?.canonicalPath
  if (left.kind === "postgres" && right.kind === "postgres") {
    return (left.viaHost ?? null) === (right.viaHost ?? null)
      && left.host === right.host
      && left.port === right.port
      && left.database === right.database
      && left.user === right.user
      && left.transportMode === right.transportMode
      && (left.insecureException?.host ?? null) === (right.insecureException?.host ?? null)
      && (left.insecureException?.user ?? null) === (right.insecureException?.user ?? null)
      && (left.insecureException?.database ?? null) === (right.insecureException?.database ?? null)
      && !!left.trustServerCertAcknowledged === !!right.trustServerCertAcknowledged
  }
  if (left.kind === "mssql" && right.kind === "mssql") {
    return (left.viaHost ?? null) === (right.viaHost ?? null)
      && left.host === right.host
      && left.port === right.port
      && left.database === right.database
      && left.user === right.user
      && left.trustCert === right.trustCert
  }
  return false
}

function NewConnectionDialog({
  open,
  mode,
  onOpenChange,
  prefill
}: {
  open: boolean
  mode: DialogMode
  onOpenChange: (open: boolean) => void
  prefill: SavedDbConnection | null
}) {
  const { t } = useTranslation("workbench")
  const openConfig = useDbStore((s) => s.openConfig)
  const updateSaved = useDbStore((s) => s.updateSaved)
  const removeCredential = useDbStore((s) => s.removeCredential)
  const openOrReconnectSavedConnection = useDbStore((s) => s.openOrReconnectSavedConnection)
  const editEntry = mode === "edit" ? prefill : null
  const reconnectEntry = mode === "reconnect" ? prefill : null
  const isEdit = editEntry !== null
  const hasStoredEditCredential = editEntry !== null
    && editEntry.kind !== "sqlite"
    && editEntry.credentialState !== "required"
    && editEntry.credentialState !== "unavailable"
  // Only a password-only reconnect locks the connection fields; edit unlocks them.
  const lockConnFields = reconnectEntry !== null
  const [kind, setKind] = useState<DbKind>(prefill?.kind ?? "sqlite")
  const [path, setPath] = useState(prefill?.path ?? "")
  const [sqliteWorkspace, setSqliteWorkspace] = useState(prefill?.workspace)
  const [host, setHost] = useState(prefill?.host ?? "")
  const [viaHost, setViaHost] = useState(prefill?.viaHost ?? "")
  const hosts = useHostStore((s) => s.configs)
  const [port, setPort] = useState(prefill ? String(prefill.port ?? defaultPort(prefill.kind)) : String(defaultPort("sqlite")))
  const [database, setDatabase] = useState(prefill?.database ?? "")
  const [user, setUser] = useState(prefill?.user ?? "")
  const [password, setPassword] = useState("")
  const [credentialAction, setCredentialAction] = useState<EditCredentialAction>(() =>
    hasStoredEditCredential ? "keep" : "replace"
  )
  const prefillTransport = prefill?.kind === "postgres"
    ? migrateLegacyPostgresTransport(prefill)
    : {
        transportMode: DEFAULT_POSTGRES_TRANSPORT_MODE,
        insecureException: null,
        trustServerCertAcknowledged: false
      }
  const [transportMode, setTransportMode] = useState<PostgresTransportMode>(
    prefillTransport.transportMode
  )
  const [insecureException, setInsecureException] = useState(
    prefillTransport.insecureException ?? null
  )
  const [trustServerCertAcknowledged, setTrustServerCertAcknowledged] = useState(
    prefillTransport.trustServerCertAcknowledged === true
  )
  const [trustCert, setTrustCert] = useState(prefill?.trustCert ?? false)
  const [pendingTransport, setPendingTransport] = useState<PostgresTransportMode | null>(null)
  const [transportChallenge, setTransportChallenge] = useState<DbPostgresTransportChallenge | null>(null)
  const pendingTransportDecision = useRef<{
    mode: PostgresTransportMode
    action: "select" | "submit" | "test"
  } | null>(null)
  const transportAckConfirmed = useRef(false)
  const transportChallengeBusy = useRef(false)
  const [busy, setBusy] = useState(false)
  const [testBusy, setTestBusy] = useState(false)
  const [testResult, setTestResult] = useState<string | null>(null)
  const [error, setError] = useState<DbProfileUiErrorCode | null>(null)
  const operationInFlight = useRef<"submit" | "test" | null>(null)

  function changeOpen(next: boolean) {
    if (!next) setPassword("")
    onOpenChange(next)
  }

  function pickKind(next: DbKind) {
    setKind(next)
    setError(null)
    if (isEdit && next !== "sqlite" && !hasStoredEditCredential) {
      setCredentialAction("replace")
    }
    // Move the port to the new engine's default (network engines only).
    if (next !== "sqlite") setPort(String(defaultPort(next)))
    if (next === "postgres") {
      setTransportMode(DEFAULT_POSTGRES_TRANSPORT_MODE)
      setInsecureException(null)
      setTrustServerCertAcknowledged(false)
      setTransportChallenge(null)
    }
  }

  const portNum = Number.parseInt(port, 10)
  const draftTarget: DbProfileTarget = kind === "sqlite"
    ? { kind: "sqlite", path: path.trim(), ...(sqliteWorkspace ? { workspace: sqliteWorkspace } : {}) }
    : kind === "postgres"
      ? {
          kind: "postgres",
          ...(viaHost ? { viaHost } : {}),
          host: host.trim(),
          port: portNum,
          database: database.trim(),
          user: user.trim(),
          transportMode,
          insecureException:
            transportMode === "insecurePlaintext"
            && postgresInsecureExceptionMatches(
              insecureException,
              host.trim(),
              portNum,
              user.trim(),
              database.trim()
            )
              ? {
                  host: host.trim(),
                  port: portNum,
                  user: user.trim(),
                  database: database.trim()
                }
              : null,
          trustServerCertAcknowledged:
            transportMode === "encryptedTrustServerCert" && trustServerCertAcknowledged
        }
      : {
          kind: "mssql",
          ...(viaHost ? { viaHost } : {}),
          host: host.trim(),
          port: portNum,
          database: database.trim(),
          user: user.trim(),
          trustCert
        }
  const persistedTarget = editEntry ? savedProfileTarget(editEntry) : null
  const editTargetChanged = editEntry !== null
    && (persistedTarget === null || !sameProfileTarget(persistedTarget, draftTarget))
  const editReplacesCredential = isEdit
    && draftTarget.kind !== "sqlite"
    && credentialAction === "replace"
  const testNeedsReplacementCredential = isEdit
    && draftTarget.kind !== "sqlite"
    && (editTargetChanged || editReplacesCredential)
    && password.length === 0
  const validConfig =
    (kind === "sqlite"
      ? path.trim().length > 0 && (!sqliteWorkspace || (path.trim().startsWith("/") && sqliteWorkspace.canonicalPath.startsWith("/")))
      : host.trim().length > 0 &&
        user.trim().length > 0 &&
        ((isEdit && credentialAction !== "replace") || password.length > 0) &&
        Number.isFinite(portNum) &&
        portNum > 0 &&
        portNum <= 65535)
  const canSave = validConfig && !busy && !testBusy
  const canTest = validConfig && !busy && !testBusy && !testNeedsReplacementCredential

  useEffect(() => {
    setTransportChallenge((current) => {
      if (!current) return current
      if (
        current.transportMode === transportMode
        && (current.viaHost ?? "") === viaHost
        && current.host === host.trim()
        && current.port === portNum
        && current.user === user.trim()
        && current.database === database.trim()
      ) {
        return current
      }
      return null
    })
  }, [transportMode, host, portNum, user, database, viaHost])

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
    } catch (error) {
      // A null result above is an ordinary user cancel. Rejections are surfaced
      // through the stable, localized profile error contract.
      setError(dbProfileUiErrorCode(error))
    }
  }

  function acknowledgedPostgresTarget(mode: PostgresTransportMode): Extract<DbProfileTarget, { kind: "postgres" }> {
    return {
      kind: "postgres",
      ...(viaHost ? { viaHost } : {}),
      host: host.trim(),
      port: portNum,
      database: database.trim(),
      user: user.trim(),
      transportMode: mode,
      insecureException: mode === "insecurePlaintext"
        ? {
            host: host.trim(),
            port: portNum,
            user: user.trim(),
            database: database.trim()
          }
        : null,
      trustServerCertAcknowledged: mode === "encryptedTrustServerCert"
    }
  }

  function buildConfig(target: DbProfileTarget = draftTarget): DbOpenConfig {
    return target.kind === "sqlite"
      ? target
      : {
          ...target,
          password: isEdit && credentialAction !== "replace" ? "" : password
        }
  }

  function challengeMatchesTarget(
    challenge: DbPostgresTransportChallenge | null,
    target: DbProfileTarget
  ): challenge is DbPostgresTransportChallenge {
    return target.kind === "postgres"
      && challenge !== null
      && postgresTransportIdentityMatches(challenge, target)
  }

  function persistedPostgresAuthorizes(target: DbProfileTarget): boolean {
    return target.kind === "postgres"
      && persistedTarget?.kind === "postgres"
      && postgresTransportAcknowledged(persistedTarget)
      && postgresTransportIdentityMatches(persistedTarget, target)
  }

  function consumeMatchingChallenge(
    target: DbProfileTarget,
    overrideId?: string
  ): string | undefined {
    if (overrideId) {
      setTransportChallenge(null)
      return overrideId
    }
    if (challengeMatchesTarget(transportChallenge, target)) {
      const id = transportChallenge.challengeId
      setTransportChallenge(null)
      return id
    }
    return undefined
  }

  function authorizePostgresOperation(
    target: DbProfileTarget,
    overrideId?: string
  ): { ok: true; challengeId?: string } | { ok: false } {
    if (target.kind !== "postgres" || target.transportMode === "verifyFull" || persistedPostgresAuthorizes(target)) {
      return { ok: true }
    }
    const challengeId = consumeMatchingChallenge(target, overrideId)
    return challengeId ? { ok: true, challengeId } : { ok: false }
  }

  function requestTransportAcknowledgement(
    mode: PostgresTransportMode,
    action: "select" | "submit" | "test"
  ) {
    pendingTransportDecision.current = { mode, action }
    transportAckConfirmed.current = false
    setPendingTransport(mode)
  }

  function requestTransportMode(next: PostgresTransportMode) {
    if (next === transportMode) return
    if (next === "verifyFull") {
      setTransportMode("verifyFull")
      setInsecureException(null)
      setTrustServerCertAcknowledged(false)
      setTransportChallenge(null)
      return
    }
    requestTransportAcknowledgement(next, "select")
  }

  function applyTransportAcknowledgement(mode: PostgresTransportMode) {
    setTransportMode(mode)
    if (mode === "insecurePlaintext") {
      setInsecureException({
        host: host.trim(),
        port: portNum,
        user: user.trim(),
        database: database.trim()
      })
      setTrustServerCertAcknowledged(false)
    } else if (mode === "encryptedTrustServerCert") {
      setInsecureException(null)
      setTrustServerCertAcknowledged(true)
    } else {
      setInsecureException(null)
      setTrustServerCertAcknowledged(false)
    }
  }

  async function confirmPendingTransport() {
    const decision = pendingTransportDecision.current
    if (!decision || transportChallengeBusy.current) return
    transportAckConfirmed.current = true
    transportChallengeBusy.current = true
    setError(null)
    try {
      const issued = await dbPostgresTransportChallenge({
        transportMode: decision.mode,
        ...(viaHost ? { viaHost } : {}),
        host: host.trim(),
        port: portNum,
        user: user.trim(),
        database: database.trim()
      })
      pendingTransportDecision.current = null
      setPendingTransport(null)
      applyTransportAcknowledgement(decision.mode)
      setTransportChallenge(issued)
      const acknowledged = acknowledgedPostgresTarget(decision.mode)
      if (decision.action === "submit") void submit(acknowledged, issued.challengeId)
      if (decision.action === "test") void testConnection(acknowledged, issued.challengeId)
    } catch (e) {
      transportAckConfirmed.current = false
      setError(dbProfileUiErrorCode(e))
    } finally {
      transportChallengeBusy.current = false
    }
  }

  function cancelPendingTransport() {
    if (transportAckConfirmed.current) {
      transportAckConfirmed.current = false
      return
    }
    pendingTransportDecision.current = null
    setPendingTransport(null)
  }

  async function submit(targetOverride?: DbProfileTarget, challengeIdOverride?: string) {
    if (!canSave || operationInFlight.current !== null) return
    const target = targetOverride ?? draftTarget
    const authorized = authorizePostgresOperation(target, challengeIdOverride)
    if (!authorized.ok) {
      requestTransportAcknowledgement(
        target.kind === "postgres" ? target.transportMode : transportMode,
        "submit"
      )
      return
    }
    operationInFlight.current = "submit"
    const config = buildConfig(target)
    setBusy(true)
    setError(null)
    try {
      const challengeOptions = authorized.challengeId
        ? { transportChallengeId: authorized.challengeId }
        : undefined
      if (editEntry) {
        await updateSaved(editEntry.id, config, challengeOptions)
        if (credentialAction === "remove" && config.kind !== "sqlite") {
          await removeCredential(editEntry.id)
        }
      } else if (reconnectEntry) {
        await updateSaved(reconnectEntry.id, config, challengeOptions)
        const opened = await openOrReconnectSavedConnection(reconnectEntry.id)
        if (opened.outcome === "error") throw opened.error
      } else {
        await openConfig(config, challengeOptions)
      }
      setPassword("")
      changeOpen(false)
    } catch (e) {
      setError(dbProfileUiErrorCode(e))
    } finally {
      operationInFlight.current = null
      setBusy(false)
    }
  }

  async function testConnection(targetOverride?: DbProfileTarget, challengeIdOverride?: string) {
    if (!canTest || operationInFlight.current !== null) return
    const target = targetOverride ?? draftTarget
    const authorized = authorizePostgresOperation(target, challengeIdOverride)
    if (!authorized.ok) {
      requestTransportAcknowledgement(
        target.kind === "postgres" ? target.transportMode : transportMode,
        "test"
      )
      return
    }
    operationInFlight.current = "test"
    setTestBusy(true)
    setError(null)
    setTestResult(null)
    try {
      const result = editEntry
        && !editTargetChanged
        && !targetOverride
        && credentialAction !== "replace"
        ? await dbTestConnection({
            kind: "saved",
            descriptorId: editEntry.id as DbDescriptorId
          })
        : await dbTestConnection({
            kind: "ephemeral",
            target,
            credential: kind === "sqlite" ? null : { password },
            ...(authorized.challengeId ? { transportChallengeId: authorized.challengeId } : {})
          })
      setTestResult(
        t("database.testConnectionSuccess", {
          elapsed: result.elapsedMs,
          version: result.serverVersion ?? t("database.testConnectionUnknownVersion")
        })
      )
    } catch (e) {
      setError(dbProfileUiErrorCode(e))
    } finally {
      operationInFlight.current = null
      setTestBusy(false)
    }
  }

  const transportAckOpen = pendingTransport === "insecurePlaintext"
    || pendingTransport === "encryptedTrustServerCert"

  return (
    <>
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        className="flex max-h-[calc(100vh-2rem)] min-h-0 flex-col gap-0 overflow-hidden p-0"
        data-testid="database-connection-dialog"
      >
        <DialogHeader className="shrink-0 px-4 pt-4">
          <DialogTitle>
            {isEdit
              ? t("database.editConnectionDialogTitle")
              : reconnectEntry
                ? t("database.reconnectDialogTitle")
                : t("database.newConnectionDialogTitle")}
          </DialogTitle>
          <DialogDescription>
            {reconnectEntry
              ? t("database.reconnectDialogDescription", { name: reconnectEntry.name })
              : isEdit
                ? t("database.editConnectionDialogDescription")
                : t("database.newConnectionDialogDescription")}
          </DialogDescription>
        </DialogHeader>
        <ScrollArea
          data-testid="database-connection-body"
          className="min-h-0 flex-1"
          viewportClassName="px-4 py-3"
          contentClassName="flex flex-col gap-[10px]"
        >
          {!lockConnFields && (
            <fieldset className="flex flex-col gap-[4px]">
              <legend className="text-[11px] font-medium text-(--ink-3)">
                {t("database.fieldEngine")}
              </legend>
              <div className="flex gap-[6px]">
                <KindChoice label="SQLite" active={kind === "sqlite"} onClick={() => pickKind("sqlite")} />
                <KindChoice label="PostgreSQL" active={kind === "postgres"} onClick={() => pickKind("postgres")} />
                <KindChoice label="MSSQL" active={kind === "mssql"} onClick={() => pickKind("mssql")} />
              </div>
            </fieldset>
          )}

          {kind === "sqlite" ? (
            <SqliteLocationFields path={path} onPathChange={setPath} workspace={sqliteWorkspace} onWorkspaceChange={setSqliteWorkspace} disabled={lockConnFields || busy || testBusy} onBrowseLocal={() => void browseSqlite()} />
          ) : (
            <>
              <FieldGroup>
                <FormField>
                  <FieldLabel htmlFor="database-via-host">{t("database.viaHost")}</FieldLabel>
                  <Select
                    value={viaHost ? `host:${viaHost}` : "direct"}
                    disabled={lockConnFields || busy || testBusy}
                    onValueChange={(value) => {
                      setViaHost(value === "direct" ? "" : value.slice(5))
                      setTransportChallenge(null)
                      setTransportMode("verifyFull")
                      setInsecureException(null)
                      setTrustServerCertAcknowledged(false)
                    }}
                  >
                    <SelectTrigger id="database-via-host"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="direct">{t("database.viaHostDirect")}</SelectItem>
                        {Object.values(hosts).map((entry) => <SelectItem key={entry.hostId} value={`host:${entry.hostId}`}>{entry.label}</SelectItem>)}
                        {viaHost && !hosts[viaHost] && <SelectItem value={`host:${viaHost}`} disabled>{t("database.viaHostMissing", { host: viaHost })}</SelectItem>}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  <FieldDescription>{t("database.viaHostDescription")}</FieldDescription>
                </FormField>
              </FieldGroup>
              <div className="flex gap-[8px]">
                <Field label={t("database.fieldHost")} className="flex-1">
                  <Input
                    value={host}
                    onChange={(e) => setHost(e.target.value)}
                    placeholder={t("database.hostPlaceholder")}
                    readOnly={lockConnFields}
                    autoFocus={!lockConnFields}
                  />
                </Field>
                <Field label={t("database.fieldPort")} className="w-[84px]">
                  <Input
                    value={port}
                    onChange={(e) => setPort(e.target.value)}
                    inputMode="numeric"
                    readOnly={lockConnFields}
                  />
                </Field>
              </div>
              <Field label={t("database.fieldDatabase")}>
                <Input
                  value={database}
                  onChange={(e) => setDatabase(e.target.value)}
                  placeholder={t("database.databasePlaceholder")}
                  readOnly={lockConnFields}
                />
              </Field>
              <Field label={t("database.fieldUser")}>
                <Input
                  value={user}
                  onChange={(e) => setUser(e.target.value)}
                  placeholder={t("database.userPlaceholder")}
                  readOnly={lockConnFields}
                />
              </Field>
              {isEdit && (
                <fieldset className="flex flex-col gap-[5px]">
                  <legend className="text-[11px] font-medium text-(--ink-3)">
                    {t("database.credentialLifecycle")}
                  </legend>
                  <label className="flex items-start gap-[7px] rounded-[7px] border border-(--line-1) px-[8px] py-[6px] text-[12px] text-(--ink-2)">
                    <input
                      type="radio"
                      name="credential-action"
                      value="keep"
                      checked={credentialAction === "keep"}
                      disabled={!hasStoredEditCredential}
                      onChange={() => {
                        setCredentialAction("keep")
                        setPassword("")
                      }}
                    />
                    <span className="flex min-w-0 flex-col">
                      <span className="font-medium">{t("database.keepCredential")}</span>
                      <span className="text-[10.5px] text-(--ink-4)">{t("database.keepCredentialDescription")}</span>
                    </span>
                  </label>
                  <label className="flex items-start gap-[7px] rounded-[7px] border border-(--line-1) px-[8px] py-[6px] text-[12px] text-(--ink-2)">
                    <input
                      type="radio"
                      name="credential-action"
                      value="replace"
                      checked={credentialAction === "replace"}
                      onChange={() => setCredentialAction("replace")}
                    />
                    <span className="flex min-w-0 flex-col">
                      <span className="font-medium">{t("database.replaceCredential")}</span>
                      <span className="text-[10.5px] text-(--ink-4)">{t("database.replaceCredentialDescription")}</span>
                    </span>
                  </label>
                  <label className="flex items-start gap-[7px] rounded-[7px] border border-(--line-1) px-[8px] py-[6px] text-[12px] text-(--ink-2)">
                    <input
                      type="radio"
                      name="credential-action"
                      value="remove"
                      checked={credentialAction === "remove"}
                      disabled={!hasStoredEditCredential}
                      onChange={() => {
                        setCredentialAction("remove")
                        setPassword("")
                      }}
                    />
                    <span className="flex min-w-0 flex-col">
                      <span className="font-medium">{t("database.removeCredentialChoice")}</span>
                      <span className="text-[10.5px] text-(--ink-4)">{t("database.removeCredentialDescription")}</span>
                    </span>
                  </label>
                </fieldset>
              )}
              {(!isEdit || credentialAction === "replace") && (
                <Field label={t("database.fieldPassword")}>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoFocus={lockConnFields}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault()
                        void submit()
                      }
                    }}
                  />
                </Field>
              )}
              {kind === "postgres" ? (
                <fieldset className="flex flex-col gap-[5px]">
                  <legend className="text-[11px] font-medium text-(--ink-3)">
                    {t("database.transportMode")}
                  </legend>
                  {([
                    ["verifyFull", "transportVerifyFull", "transportVerifyFullDescription"],
                    ["encryptedTrustServerCert", "transportTrustServerCert", "transportTrustServerCertDescription"],
                    ["insecurePlaintext", "transportInsecurePlaintext", "transportInsecurePlaintextDescription"]
                  ] as const).map(([mode, labelKey, descriptionKey]) => (
                    <label
                      key={mode}
                      className="flex items-start gap-[7px] rounded-[7px] border border-(--line-1) px-[8px] py-[6px] text-[12px] text-(--ink-2)"
                    >
                      <input
                        type="radio"
                        name="postgres-transport"
                        value={mode}
                        checked={transportMode === mode}
                        onChange={() => requestTransportMode(mode)}
                      />
                      <span className="flex min-w-0 flex-col">
                        <span className="font-medium">{t(`database.${labelKey}`)}</span>
                        <span className="text-[10.5px] text-(--ink-4)">{t(`database.${descriptionKey}`)}</span>
                      </span>
                    </label>
                  ))}
                  {transportMode === "insecurePlaintext" && (
                    <p role="note" className="rounded-[6px] bg-(--danger-soft) px-[8px] py-[6px] text-[11px] text-(--destructive)">
                      {t("database.transportPlaintextWarning")}
                    </p>
                  )}
                  {transportMode === "encryptedTrustServerCert" && (
                    <p role="note" className="rounded-[6px] bg-(--amber-soft) px-[8px] py-[6px] text-[11px] text-(--ink-2)">
                      {t("database.transportTrustServerCertWarning")}
                    </p>
                  )}
                </fieldset>
              ) : (
                <label className="flex items-center gap-[7px] text-[12px] text-(--ink-2)">
                  <Checkbox
                    checked={trustCert}
                    onCheckedChange={(checked) => setTrustCert(checked === true)}
                  />
                  {t("database.trustCert")}
                </label>
              )}
            </>
          )}

          {error && (
            <p role="alert" className="rounded-[6px] bg-(--danger-soft) px-[8px] py-[6px] font-mono text-[11px] whitespace-pre-wrap text-(--destructive)">
              {t(`database.profileError.${error}`)}
            </p>
          )}
          {testResult && (
            <p role="status" className="rounded-[6px] bg-(--yz-hover) px-[8px] py-[6px] text-[11px] text-(--term-ok)">
              {testResult}
            </p>
          )}
          {testNeedsReplacementCredential && (
            <p role="note" className="rounded-[6px] bg-(--amber-soft) px-[8px] py-[6px] text-[11px] text-(--ink-2)">
              {t("database.testConnectionChangedTargetPasswordRequired")}
            </p>
          )}
        </ScrollArea>
        <DialogFooter className="mx-0 mb-0 shrink-0 rounded-none border-t px-4 py-4">
          <Button variant="outline" onClick={() => changeOpen(false)}>
            {t("database.cancel")}
          </Button>
          <Button variant="outline" onClick={() => void testConnection()} disabled={!canTest}>
            {testBusy ? t("database.testingConnection") : t("database.testConnection")}
          </Button>
          <Button onClick={() => void submit()} disabled={!canSave}>
            {isEdit
              ? t("database.save")
              : reconnectEntry
                ? t("database.connect")
                : t("database.saveAndConnect")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    <AlertDialog
      open={transportAckOpen}
      onOpenChange={(next) => {
        if (!next) cancelPendingTransport()
      }}
    >
      <AlertDialogContent data-testid="database-transport-ack-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {pendingTransport === "insecurePlaintext"
              ? t("database.transportPlaintextTitle")
              : t("database.transportTrustServerCertTitle")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t(
              pendingTransport === "insecurePlaintext"
                ? "database.transportPlaintextBody"
                : "database.transportTrustServerCertBody",
              {
                host: host.trim() || t("database.transportUnknownHost"),
                user: user.trim() || t("database.transportUnknownUser"),
                database: database.trim() || t("database.transportUnknownDatabase")
              }
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={cancelPendingTransport}>
            {t("database.cancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            onPointerDown={(event) => {
              event.preventDefault()
              void confirmPendingTransport()
            }}
            onClick={(event) => {
              event.preventDefault()
              void confirmPendingTransport()
            }}
          >
            {pendingTransport === "insecurePlaintext"
              ? t("database.transportPlaintextConfirm")
              : t("database.transportTrustServerCertConfirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    </>
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
      aria-pressed={active}
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
