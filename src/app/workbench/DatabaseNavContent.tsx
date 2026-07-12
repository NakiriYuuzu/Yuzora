import { useEffect, useRef, useState } from "react"
import { open as openFileDialog } from "@tauri-apps/plugin-dialog"
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Database,
  Eye,
  History,
  KeyRound,
  Pencil,
  Table2,
  Trash2,
  X
} from "lucide-react"
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
import { dbObjectRefKey } from "@/lib/databaseSql"
import { dbTestConnection } from "@/lib/ipc"
import { relativeTime } from "@/lib/relativeTime"
import type {
  DbDescriptorId,
  DbKind,
  DbOpenConfig,
  DbProfileTarget,
  DbTable
} from "@/lib/types"
import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import {
  dbProfileNeedsCredentialPrompt,
  dbProfileUiErrorCode,
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
    <div data-testid="db-nav-root" className="flex h-full min-h-0 flex-col gap-[8px] overflow-hidden">
      {(visibleProfileError || actionError) && (
        <p role="alert" className="rounded-[8px] bg-(--danger-soft) px-[8px] py-[6px] text-[11px] text-(--destructive)">
          {actionError ? t(`database.profileError.${actionError}`) : visibleProfileError}
        </p>
      )}
      {recovery.length > 0 && (
        <section aria-label={t("database.recoveryHeading")} className="flex max-h-[96px] shrink-0 flex-col gap-[4px] overflow-y-auto rounded-[8px] border border-(--line-1) p-[7px]">
          <span className="flex items-center gap-[5px] text-[10px] font-semibold tracking-[0.06em] text-(--ink-3) uppercase">
            <AlertTriangle className="size-[12px]" aria-hidden="true" />
            {t("database.recoveryHeading")}
          </span>
          {recovery.map((row) => (
            <div key={row.operationId} className="flex items-center gap-[5px] text-[11px] text-(--ink-2)">
              <span className="min-w-0 flex-1 truncate">
                {t(`database.recoveryKind.${row.kind}`)} · {row.descriptorId}
              </span>
              {row.allowedActions.includes("resume") && (
                <Button
                  size="sm"
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
                  size="sm"
                  variant="ghost"
                  disabled={recoveryBusy}
                  onClick={() => void executeRecoveryAction(row.operationId, "abort")}
                >
                  {t("database.recoveryAbort")}
                </Button>
              )}
              {row.allowedActions.includes("retryCleanup") && (
                <Button
                  size="sm"
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
      )}
      <div
        data-testid="db-region-grid"
        className="grid min-h-0 flex-1 grid-rows-[minmax(40px,0.8fr)_minmax(40px,1.4fr)_minmax(24px,0.7fr)] gap-[8px] overflow-hidden"
      >
        <SavedConnectionsRegion onOpenEdit={openEdit} onError={setActionError} />
        <DatabaseObjectTreeRegion />
        <RecentQueriesRegion />
      </div>

      <div data-testid="db-new-connection" className="shrink-0">
        <DashedActionButton label={t("database.newConnection")} onClick={openNew} />
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
        <DialogContent className="sm:max-w-[400px]">
          <DialogHeader>
            <DialogTitle>{t("database.recoveryCredentialTitle")}</DialogTitle>
            <DialogDescription>{t("database.recoveryCredentialDescription")}</DialogDescription>
          </DialogHeader>
          <Input
            type="password"
            autoComplete="new-password"
            value={recoveryPassword}
            onChange={(event) => setRecoveryPassword(event.target.value)}
            aria-label={t("database.fieldPassword")}
          />
          <DialogFooter>
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
      <h2
        id="db-saved-heading"
        className="shrink-0 px-[8px] pb-[3px] text-[10px] font-semibold tracking-[0.08em] text-(--ink-4) uppercase"
      >
        {t("database.savedConnectionsHeading")}
      </h2>
      <div
        data-testid="db-saved-scroll"
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
      >
        {saved.length === 0 ? (
          <div className="flex min-h-[72px] items-center justify-center px-[8px]">
            <EmptyState
              icon={Database}
              title={t("database.emptyTitle")}
              description={t("database.emptyDescription")}
            />
          </div>
        ) : (
          <ul className="flex flex-col gap-[2px]">
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
                      "group flex min-h-[38px] items-center gap-[7px] rounded-[8px] px-[8px] py-[3px] text-[12.5px] transition-colors",
                      isActive
                        ? "bg-(--yz-solid) text-(--ink-1)"
                        : "text-(--ink-2) hover:bg-(--yz-hover)"
                    )}
                  >
                    <button
                      type="button"
                      aria-current={isActive ? "true" : undefined}
                      onClick={() => void openOrReconnectSavedConnection(entry.id)}
                      title={savedConnectionAddress(entry)}
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
                          {credentialNote ? ` · ${credentialNote}` : ""}
                        </span>
                      </span>
                    </button>
                    <StatusBadge status={status} error={session?.error ?? null} />
                    {confirmDeleteId === entry.id ? (
                      <div className="flex shrink-0 items-center gap-[2px]">
                        <button
                          type="button"
                          aria-label={t("database.confirmRemove", { name: entry.name })}
                          onClick={() => {
                            setConfirmDeleteId(null)
                            void removeSaved(entry.id).catch((error) => {
                              onError(dbProfileUiErrorCode(error))
                            })
                          }}
                          className="flex size-[18px] items-center justify-center rounded-[5px] text-(--destructive) transition-colors hover:bg-(--danger-soft)"
                        >
                          <Check className="size-[12px]" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          aria-label={t("database.cancelRemove")}
                          onClick={() => setConfirmDeleteId(null)}
                          className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                        >
                          <X className="size-[12px]" aria-hidden="true" />
                        </button>
                      </div>
                    ) : (
                      <div className="flex shrink-0 items-center gap-[2px] opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
                        {entry.kind !== "sqlite" && entry.credentialState === "stored" && (
                          <button
                            type="button"
                            aria-label={t("database.removeCredential", { name: entry.name })}
                            onClick={() => void removeCredential(entry.id).catch((error) => {
                              onError(dbProfileUiErrorCode(error))
                            })}
                            className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                          >
                            <KeyRound className="size-[12px]" aria-hidden="true" />
                          </button>
                        )}
                        <button
                          type="button"
                          aria-label={t("database.editConnection", { name: entry.name })}
                          onClick={() => onOpenEdit(entry)}
                          className="flex size-[18px] items-center justify-center rounded-[5px] text-(--ink-4) transition-colors hover:bg-(--yz-hover) hover:text-(--ink-1)"
                        >
                          <Pencil className="size-[12px]" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          aria-label={t("database.forgetConnection", { name: entry.name })}
                          onClick={() => setConfirmDeleteId(entry.id)}
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
      </div>
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
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set())
  const [expandedObjects, setExpandedObjects] = useState<Set<string>>(() => new Set())

  const activeConn = connections.find((connection) =>
    connection.descriptorId === activeDescriptorId
  )
  const activeTables = activeDescriptorId
    ? (tableBuckets[activeDescriptorId] ?? (activeConnId ? tables[activeConnId] : undefined) ?? [])
    : []
  const activeTableError = activeDescriptorId ? tableErrors[activeDescriptorId] ?? null : null
  const groups = groupDatabaseObjects(activeTables)

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
    const isExpanded = expandedObjects.has(expansionKey)
    setExpandedObjects((current) => {
      const next = new Set(current)
      if (isExpanded) next.delete(expansionKey)
      else next.add(expansionKey)
      return next
    })
    if (
      !isExpanded
      && columnBuckets[activeDescriptorId]?.[refKey] === undefined
      && !columnErrors[activeDescriptorId]?.[refKey]
    ) {
      void loadColumns(activeDescriptorId, object)
    }
  }

  return (
    <section
      data-testid="db-object-region"
      aria-labelledby="db-object-heading"
      className="flex min-h-0 flex-col overflow-hidden"
    >
      <h2
        id="db-object-heading"
        className="shrink-0 px-[8px] pb-[3px] text-[10px] font-semibold tracking-[0.08em] text-(--ink-4) uppercase"
      >
        {t("database.objectTreeHeading")}
      </h2>
      <div
        data-testid="db-object-scroll"
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
      >
        {activeTableError && activeDescriptorId && (
          <div
            role="alert"
            className="mx-[4px] mb-[4px] flex items-start gap-[7px] rounded-[7px] border border-(--line-1) bg-(--amber-soft) px-[8px] py-[6px] text-[11px] text-(--ink-2)"
          >
            <span className="min-w-0 flex-1">
              {activeTableError.code === "connectionBusy"
                ? t("database.tableConnectionBusy")
                : t("database.tableRefreshFailed")}
            </span>
            <Button
              size="sm"
              variant="outline"
              aria-label={t("database.retryObjectRefresh")}
              onClick={() => void loadTables(activeDescriptorId)}
            >
              {t("database.retry")}
            </Button>
          </div>
        )}
        {!activeConn || !activeDescriptorId ? (
          <p className="px-[8px] py-[4px] text-[12px] text-(--ink-4)">
            {t("database.noActiveConnectionObjects")}
          </p>
        ) : groups.length === 0 ? (
          <p className="px-[8px] py-[4px] text-[12px] text-(--ink-4)">{t("database.noTables")}</p>
        ) : (
          <ul className="flex flex-col gap-[1px]">
            {groups.map((catalogGroup) => {
              const catalogKey = JSON.stringify([activeDescriptorId, "catalog", catalogGroup.catalog])
              const catalogExpanded = !collapsedGroups.has(catalogKey)
              return (
                <li key={catalogKey}>
                  <TreeGroupToggle
                    expanded={catalogExpanded}
                    label={t("database.catalogGroup", { name: catalogGroup.catalog })}
                    level="catalog"
                    groupKey={catalogKey}
                    onClick={() => toggleGroup(catalogKey)}
                  />
                  {catalogExpanded && (
                    <ul className="pl-[9px]">
                      {catalogGroup.schemas.map((schemaGroup) => {
                        const schemaKey = JSON.stringify([
                          activeDescriptorId,
                          "schema",
                          catalogGroup.catalog,
                          schemaGroup.schema
                        ])
                        const schemaExpanded = !collapsedGroups.has(schemaKey)
                        return (
                          <li key={schemaKey}>
                            <TreeGroupToggle
                              expanded={schemaExpanded}
                              label={t("database.schemaGroup", { name: schemaGroup.schema })}
                              level="schema"
                              groupKey={schemaKey}
                              onClick={() => toggleGroup(schemaKey)}
                            />
                            {schemaExpanded && (
                              <ul className="pl-[9px]">
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
                                  const kindExpanded = !collapsedGroups.has(kindKey)
                                  return (
                                    <li key={kindKey}>
                                      <TreeGroupToggle
                                        expanded={kindExpanded}
                                        label={kind === "table"
                                          ? t("database.tableGroupHeading")
                                          : t("database.viewGroupHeading")}
                                        level="kind"
                                        groupKey={kindKey}
                                        onClick={() => toggleGroup(kindKey)}
                                      />
                                      {kindExpanded && (
                                        <ul className="pl-[9px]">
                                          {objects.map((object) => {
                                            const refKey = dbObjectRefKey(object)
                                            const expansionKey = `${activeDescriptorId}:${refKey}`
                                            const objectExpanded = expandedObjects.has(expansionKey)
                                            const columns = columnBuckets[activeDescriptorId]?.[refKey]
                                            const columnError = columnErrors[activeDescriptorId]?.[refKey] ?? null
                                            const Icon = object.kind === "view" ? Eye : Table2
                                            return (
                                              <li
                                                key={refKey}
                                                data-testid="db-object-row"
                                                data-object-ref={refKey}
                                              >
                                                <div className="flex h-[26px] items-center rounded-[7px] text-(--ink-2) hover:bg-(--yz-hover) hover:text-(--ink-1)">
                                                  <button
                                                    type="button"
                                                    aria-label={objectExpanded
                                                      ? t("database.collapseColumns", { name: object.name })
                                                      : t("database.expandColumns", { name: object.name })}
                                                    aria-expanded={objectExpanded}
                                                    onClick={() => toggleObject(object)}
                                                    className="flex size-[24px] shrink-0 items-center justify-center rounded-[6px] text-(--ink-4) hover:text-(--ink-1)"
                                                  >
                                                    {objectExpanded
                                                      ? <ChevronDown className="size-[12px]" aria-hidden="true" />
                                                      : <ChevronRight className="size-[12px]" aria-hidden="true" />}
                                                  </button>
                                                  <button
                                                    type="button"
                                                    onClick={() => void openTableQuery(object)}
                                                    title={`${object.catalog}.${object.schema}.${object.name}`}
                                                    className="flex min-w-0 flex-1 items-center gap-[7px] pr-[8px] text-left text-[12.5px]"
                                                  >
                                                    <Icon className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                                                    <span className="truncate font-mono">{object.name}</span>
                                                  </button>
                                                </div>
                                                {objectExpanded && (
                                                  <div className="ml-[24px] border-l border-(--line-1) pl-[8px]">
                                                    {columnError ? (
                                                      <div role="alert" className="flex items-start gap-[5px] py-[4px] text-[10.5px] text-(--ink-3)">
                                                        <span className="min-w-0 flex-1">
                                                          {columnError.code === "connectionBusy"
                                                            ? t("database.tableConnectionBusy")
                                                            : t("database.columnRefreshFailed")}
                                                        </span>
                                                        <button
                                                          type="button"
                                                          aria-label={t("database.retryColumns", { name: object.name })}
                                                          onClick={() => void loadColumns(activeDescriptorId, object)}
                                                          className="shrink-0 rounded-[5px] px-[5px] py-[1px] text-(--yz-accent-ink) hover:bg-(--yz-hover)"
                                                        >
                                                          {t("database.retry")}
                                                        </button>
                                                      </div>
                                                    ) : columns === undefined ? (
                                                      <p role="status" className="py-[3px] text-[10.5px] text-(--ink-4)">
                                                        {t("database.loadingColumns")}
                                                      </p>
                                                    ) : columns.length === 0 ? (
                                                      <p className="py-[3px] text-[10.5px] text-(--ink-4)">
                                                        {t("database.noColumns")}
                                                      </p>
                                                    ) : (
                                                      <ul>
                                                        {columns.map((column) => (
                                                          <li
                                                            key={column.name}
                                                            className="flex min-h-[22px] items-center gap-[5px] py-[2px] font-mono text-[10.5px] text-(--ink-3)"
                                                          >
                                                            <span className="min-w-0 flex-1 truncate text-(--ink-2)">
                                                              {column.name}
                                                            </span>
                                                            <span className="max-w-[84px] truncate">{column.type}</span>
                                                            <span>{column.notnull
                                                              ? t("database.columnNotNull")
                                                              : t("database.columnNullable")}</span>
                                                            {column.pk && <span>{t("database.columnPrimaryKey")}</span>}
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
      </div>
    </section>
  )
}

function TreeGroupToggle({
  expanded,
  label,
  level,
  groupKey,
  onClick
}: {
  expanded: boolean
  label: string
  level: "catalog" | "schema" | "kind"
  groupKey: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      data-testid="db-object-group-toggle"
      data-group-level={level}
      data-group-key={groupKey}
      aria-expanded={expanded}
      onClick={onClick}
      className="flex h-[24px] w-full items-center gap-[4px] rounded-[6px] px-[4px] text-left text-[11px] font-medium text-(--ink-3) hover:bg-(--yz-hover) hover:text-(--ink-1)"
    >
      {expanded
        ? <ChevronDown className="size-[12px] shrink-0" aria-hidden="true" />
        : <ChevronRight className="size-[12px] shrink-0" aria-hidden="true" />}
      <span className="truncate">{label}</span>
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
      className="flex max-h-[168px] min-h-0 flex-col overflow-hidden"
    >
      <button
        id="db-history-heading"
        type="button"
        data-testid="db-history-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
        className="flex h-[24px] shrink-0 items-center gap-[5px] rounded-[6px] px-[8px] text-left text-[10px] font-semibold tracking-[0.08em] text-(--ink-4) uppercase hover:bg-(--yz-hover) hover:text-(--ink-2)"
      >
        {expanded
          ? <ChevronDown className="size-[12px]" aria-hidden="true" />
          : <ChevronRight className="size-[12px]" aria-hidden="true" />}
        <History className="size-[12px]" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">{t("database.recentQueriesHeading")}</span>
        <span className="font-mono text-[9px]" aria-hidden="true">{historyEntries.length}</span>
      </button>
      {expanded && (
        <div
          data-testid="db-history-scroll"
          className="min-h-0 max-h-[144px] overflow-y-auto overscroll-contain"
        >
          {historyEntries.length === 0 ? (
            <p className="px-[8px] py-[4px] text-[11px] text-(--ink-4)">
              {t("database.noRecentQueries")}
            </p>
          ) : (
            <ul className="flex flex-col gap-[1px]">
              {historyEntries.map((entry, index) => (
                <li
                  key={`${entry.ranAt}-${index}`}
                  data-testid="db-history-row"
                >
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
          )}
        </div>
      )}
    </section>
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
    connecting: {
      label: t("database.statusConnecting"),
      className: "bg-(--amber-soft) text-(--ink-2)"
    },
    connected: {
      label: t("database.statusConnected"),
      className: "bg-(--yz-hover) text-(--term-ok)"
    },
    error: { label: t("database.statusError"), className: "bg-(--danger-soft) text-(--destructive)" },
    disconnected: {
      label: t("database.statusOffline"),
      className: "bg-(--yz-hover) text-(--ink-4)"
    }
  }
  const { label, className } = map[status]
  return (
    <span
      data-status={status === "disconnected" ? "offline" : status}
      title={status === "error" && error ? t(`database.profileError.${error}`) : undefined}
      className={cn(
        "shrink-0 rounded-(--r-pill) px-[7px] py-[1px] text-[10px] font-medium",
        className
      )}
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
    return entry.path ? { kind: "sqlite", path: entry.path } : null
  }
  if (!entry.host || !entry.port || !entry.database || !entry.user) return null
  if (entry.kind === "postgres") {
    return {
      kind: "postgres",
      host: entry.host,
      port: entry.port,
      database: entry.database,
      user: entry.user,
      ssl: entry.ssl ?? false,
      trustCert: entry.trustCert ?? false
    }
  }
  return {
    kind: "mssql",
    host: entry.host,
    port: entry.port,
    database: entry.database,
    user: entry.user,
    trustCert: entry.trustCert ?? false
  }
}

function sameProfileTarget(left: DbProfileTarget, right: DbProfileTarget): boolean {
  if (left.kind !== right.kind) return false
  if (left.kind === "sqlite" && right.kind === "sqlite") return left.path === right.path
  if (left.kind === "postgres" && right.kind === "postgres") {
    return left.host === right.host
      && left.port === right.port
      && left.database === right.database
      && left.user === right.user
      && left.ssl === right.ssl
      && left.trustCert === right.trustCert
  }
  if (left.kind === "mssql" && right.kind === "mssql") {
    return left.host === right.host
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
  const [host, setHost] = useState(prefill?.host ?? "")
  const [port, setPort] = useState(prefill ? String(prefill.port ?? defaultPort(prefill.kind)) : String(defaultPort("sqlite")))
  const [database, setDatabase] = useState(prefill?.database ?? "")
  const [user, setUser] = useState(prefill?.user ?? "")
  const [password, setPassword] = useState("")
  const [credentialAction, setCredentialAction] = useState<EditCredentialAction>(() =>
    hasStoredEditCredential ? "keep" : "replace"
  )
  const [ssl, setSsl] = useState(prefill?.ssl ?? false)
  const [trustCert, setTrustCert] = useState(prefill?.trustCert ?? false)
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
  }

  const portNum = Number.parseInt(port, 10)
  const draftTarget: DbProfileTarget = kind === "sqlite"
    ? { kind: "sqlite", path: path.trim() }
    : kind === "postgres"
      ? {
          kind: "postgres",
          host: host.trim(),
          port: portNum,
          database: database.trim(),
          user: user.trim(),
          ssl,
          trustCert
        }
      : {
          kind: "mssql",
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
      ? path.trim().length > 0
      : host.trim().length > 0 &&
        database.trim().length > 0 &&
        user.trim().length > 0 &&
        ((isEdit && credentialAction !== "replace") || password.length > 0) &&
        Number.isFinite(portNum) &&
        portNum > 0 &&
        portNum <= 65535)
  const canSave = validConfig && !busy && !testBusy
  const canTest = validConfig && !busy && !testBusy && !testNeedsReplacementCredential

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

  function buildConfig(): DbOpenConfig {
    return draftTarget.kind === "sqlite"
      ? draftTarget
      : {
          ...draftTarget,
          password: isEdit && credentialAction !== "replace" ? "" : password
        }
  }

  async function submit() {
    if (!canSave || operationInFlight.current !== null) return
    operationInFlight.current = "submit"
    const config = buildConfig()
    setBusy(true)
    setError(null)
    try {
      if (editEntry) {
        await updateSaved(editEntry.id, config)
        if (credentialAction === "remove" && config.kind !== "sqlite") {
          await removeCredential(editEntry.id)
        }
      } else if (reconnectEntry) {
        await updateSaved(reconnectEntry.id, config)
        const opened = await openOrReconnectSavedConnection(reconnectEntry.id)
        if (opened.outcome === "error") throw opened.error
      } else {
        await openConfig(config)
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

  async function testConnection() {
    if (!canTest || operationInFlight.current !== null) return
    operationInFlight.current = "test"
    setTestBusy(true)
    setError(null)
    setTestResult(null)
    try {
      const result = editEntry
        && !editTargetChanged
        && credentialAction !== "replace"
        ? await dbTestConnection({
            kind: "saved",
            descriptorId: editEntry.id as DbDescriptorId
          })
        : await dbTestConnection({
            kind: "ephemeral",
            target: draftTarget,
            credential: kind === "sqlite" ? null : { password }
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

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
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
        <div className="flex flex-col gap-[10px]">
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
            <div className="flex flex-col gap-[4px]">
              <label htmlFor="database-file-path" className="text-[11px] font-medium text-(--ink-3)">
                {t("database.fieldFile")}
              </label>
              <div className="flex gap-[6px]">
                <Input
                  id="database-file-path"
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                  placeholder={t("database.filePlaceholder")}
                  className="flex-1"
                />
                <Button type="button" variant="outline" onClick={() => void browseSqlite()}>
                  {t("database.browse")}
                </Button>
              </div>
            </div>
          ) : (
            <>
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
                <>
                  <label className="flex items-center gap-[7px] text-[12px] text-(--ink-2)">
                    <input
                      type="checkbox"
                      checked={ssl}
                      onChange={(e) => setSsl(e.target.checked)}
                    />
                    {t("database.useSsl")}
                  </label>
                  <label className="flex items-center gap-[7px] text-[12px] text-(--ink-2)">
                    <input
                      type="checkbox"
                      checked={trustCert}
                      disabled={!ssl}
                      onChange={(e) => setTrustCert(e.target.checked)}
                    />
                    {t("database.trustCert")}
                  </label>
                </>
              ) : (
                <label className="flex items-center gap-[7px] text-[12px] text-(--ink-2)">
                  <input
                    type="checkbox"
                    checked={trustCert}
                    onChange={(e) => setTrustCert(e.target.checked)}
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
        </div>
        <DialogFooter>
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
