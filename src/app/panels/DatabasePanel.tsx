import { memo, useEffect, useMemo, useRef, useState } from "react"
import type { ReactNode } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView, keymap, lineNumbers } from "@codemirror/view"
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands"
import { syntaxHighlighting } from "@codemirror/language"
import { sql } from "@codemirror/lang-sql"
import { AlertTriangle, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, CircleStop, Lock, PencilLine, Play, RefreshCw, Table2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { databaseErrorSelection, databaseErrorSelectionForEditor } from "@/lib/databaseSql"
import {
  identityOf,
  queryFor,
  queryRunGroupIsCancellable,
  resultPageStateForStatement,
  savedConnectionAddress,
  useDbStore,
} from "@/state/dbStore"
import type {
  DbQueryErrorState,
  DbQueryRunGroup,
  DbSort,
  DbStatementResultPageState,
} from "@/state/dbStore"
import { shortcutLabel } from "@/lib/platform"
import { formatDbValue } from "@/lib/types"
import type {
  DbColumn,
  DbEffectOutcome,
  DbError,
  DbQueryResult,
  DbResultSessionOwner,
  DbStatementExecution,
  DbValue,
} from "@/lib/types"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Kbd } from "@/components/ui/kbd"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from "@/components/ui/resizable"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { dbObjectRefKey } from "@/lib/databaseSql"
import { DatabaseCatalogPicker } from "./DatabaseCatalogPicker"
import { DatabaseCellEditing } from "./DatabaseCellEditing"
import { useDatabaseCellEditing } from "./databaseCellEditingContext"

const EMPTY_COLUMNS: DbColumn[] = []

/** Move the column at display position `from` to display position `to`. `order`
 *  maps display positions to original column indices; the returned array is a new
 *  ordering (input untouched). */
export function reorderColumns(order: number[], from: number, to: number): number[] {
  const next = [...order]
  const [moved] = next.splice(from, 1)
  next.splice(to, 0, moved)
  return next
}

function sameColumns(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  return a.every((v, i) => v === b[i])
}

// Sizing/font the shared appTheme doesn't set (it only carries colours). Mono
// font + fixed editor height for the SQL console.
const dbEditorTheme = EditorView.theme({
  "&": { height: "100%", fontSize: "13px" },
  ".cm-content": { fontFamily: "var(--font-mono, monospace)" },
  ".cm-gutters": { fontFamily: "var(--font-mono, monospace)" }
})

/**
 * Database mode main region (FEAT-1). With no active connection it keeps the
 * original empty state; once a connection is open it becomes a SQL console:
 * a CodeMirror editor (SQL highlight, Cmd/Ctrl+Enter to run) over a results
 * area. The panel is remounted on mode switch, but the SQL text and last
 * result live in dbStore so they survive leaving/returning to Database mode.
 */
export function DatabasePanel() {
  const { t } = useTranslation("panels")
  const descriptorId = useDbStore((state) => {
    const activeDescriptorId = state.activeDescriptorId
    if (
      activeDescriptorId &&
      state.connections.some((connection) => connection.descriptorId === activeDescriptorId)
    ) {
      return activeDescriptorId
    }
    if (activeDescriptorId) {
      const activeGroup = queryFor(state, activeDescriptorId).runGroup
      if (
        activeGroup?.run?.connectionTerminated ||
        activeGroup?.cancelOutcome === "cancelledConnectionTerminated"
      ) {
        return activeDescriptorId
      }
    }

    let newestTerminated: { descriptorId: string; startedAt: number } | null = null
    for (const [candidateDescriptorId, query] of Object.entries(state.queryBuckets)) {
      const group = query.runGroup
      if (
        !group ||
        (!group.run?.connectionTerminated &&
          group.cancelOutcome !== "cancelledConnectionTerminated")
      ) {
        continue
      }
      if (!newestTerminated || group.startedAt > newestTerminated.startedAt) {
        newestTerminated = { descriptorId: candidateDescriptorId, startedAt: group.startedAt }
      }
    }
    return newestTerminated?.descriptorId ?? null
  })
  const connected = useDbStore((state) =>
    descriptorId !== null &&
    state.connections.some((connection) => connection.descriptorId === descriptorId)
  )

  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden bg-(--paper-0)">
      {descriptorId ? (
        <DatabaseConsole
          key={`${descriptorId}:${connected ? "connected" : "offline"}`}
          descriptorId={descriptorId}
          connected={connected}
        />
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={Table2}
            title={t("databasePanel.emptyTitle")}
            description={t("databasePanel.emptyDescription")}
          />
        </div>
      )}
    </div>
  )
}

function DatabaseConsole({ descriptorId, connected }: { descriptorId: string; connected: boolean }) {
  const { t } = useTranslation("panels")
  const { t: workbench } = useTranslation("databaseWorkbench")
  const editorPanel = useRef<PanelImperativeHandle | null>(null)
  const table = useDbStore(state => queryFor(state, descriptorId).table ?? null)
  const editUnconfirmed = useDbStore(state => queryFor(state, descriptorId).editUnconfirmed)
  const [modeChoice, setModeChoice] = useState({ table, mode: table ? "data" : "query" })
  const mode = modeChoice.table === table ? modeChoice.mode : table ? "data" : "query"
  const metadata = useDbStore(state => table ? state.columnBuckets[descriptorId]?.[dbObjectRefKey(table)] ?? EMPTY_COLUMNS : EMPTY_COLUMNS)
  useEffect(() => {
    if (mode === "data") editorPanel.current?.collapse()
    else editorPanel.current?.expand()
  }, [mode])
  const containerRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const activeProfile = useDbStore((state) =>
    state.saved.find((profile) => profile.id === descriptorId) ?? null
  )
  const activeConnection = useDbStore((state) =>
    state.connections.find((connection) => connection.descriptorId === descriptorId) ?? null
  )
  const identity = useMemo(() => identityOf(activeConnection ?? undefined), [activeConnection])
  const needsDatabase = activeProfile?.kind !== "sqlite" && activeProfile?.database === ""
  const sqlText = useDbStore((state) => queryFor(state, descriptorId).sql)
  const running = useDbStore((state) => queryFor(state, descriptorId).running)
  const result = useDbStore((state) => queryFor(state, descriptorId).result)
  const error = useDbStore((state) => queryFor(state, descriptorId).error)
  const parseError = useDbStore((state) => queryFor(state, descriptorId).parseError)
  const elapsedMs = useDbStore((state) => queryFor(state, descriptorId).elapsedMs)
  const sortBy = useDbStore((state) => queryFor(state, descriptorId).sortBy)
  const runGroup = useDbStore((state) => queryFor(state, descriptorId).runGroup)
  const runQuery = useDbStore((s) => s.runQuery)
  const cancelQuery = useDbStore((s) => s.cancelQuery)
  const selectStatementTab = useDbStore((s) => s.selectStatementTab)
  const previousResultPage = useDbStore((s) => s.previousResultPage)
  const nextResultPage = useDbStore((s) => s.nextResultPage)
  const releaseResultSession = useDbStore((s) => s.releaseResultSession)
  const sortResult = useDbStore((s) => s.sortResult)

  // Mount the editor once. Seeds from the persisted sql; keystrokes flow back
  // into the store via setSql. Store actions are read through getState() so the
  // extensions never go stale (this effect runs a single time).
  useEffect(() => {
    if (!containerRef.current) return
    const st = useDbStore.getState()
    const state = EditorState.create({
      doc: queryFor(st, descriptorId).sql,
      extensions: [
        appTheme,
        dbEditorTheme,
        EditorState.readOnly.of(!connected),
        EditorView.editable.of(connected),
        lineNumbers(),
        history(),
        sql(),
        syntaxHighlighting(appHighlightStyle),
        keymap.of([
          {
            key: "Mod-Enter",
            run: (view) => {
              const selection = view.state.selection.main
              void useDbStore.getState().runQuery({
                kind: "primary",
                selection: { from: selection.from, to: selection.to },
                cursor: selection.head
              })
              return true
            }
          },
          {
            key: "Mod-Shift-Enter",
            run: () => {
              void useDbStore.getState().runQuery({ kind: "all" })
              return true
            }
          },
          ...defaultKeymap,
          ...historyKeymap
        ]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) useDbStore.getState().setSql(u.state.doc.toString())
        })
      ]
    })
    const view = new EditorView({ state, parent: containerRef.current })
    viewRef.current = view
    return () => {
      view.destroy()
      viewRef.current = null
    }
  }, [connected, descriptorId])

  // Reflect programmatic sql changes (a table click in the nav) into the editor.
  // Guarded by an equality check so a keystroke — which already updated the doc
  // before setSql ran — doesn't re-dispatch and fight the cursor.
  useEffect(() => {
    const view = viewRef.current
    if (!view) return
    const current = view.state.doc.toString()
    if (current === sqlText) return
    view.dispatch({ changes: { from: 0, to: current.length, insert: sqlText } })
  }, [sqlText])

  useEffect(() => {
    const view = viewRef.current
    if (!view || !parseError) return
    const docLength = view.state.doc.length
    const from = Math.max(0, Math.min(parseError.from, docLength))
    const to = Math.max(from, Math.min(parseError.to, docLength))
    view.dispatch({
      selection: { anchor: from, head: to }
    })
  }, [parseError])

  useEffect(() => {
    const view = viewRef.current
    if (!view || parseError || !runGroup || runGroup.units.length === 0) return
    const activeStatement = runGroup.run?.statements.find((statement) =>
      statement.statementExecutionId === runGroup.activeStatementExecutionId
    )
    const activeUnit = activeStatement ? runGroup.units[activeStatement.statementIndex] : null
    const firstUnit = activeUnit ?? runGroup.units[0]
    const lastUnit = activeUnit ?? runGroup.units[runGroup.units.length - 1]
    if (!firstUnit || !lastUnit || lastUnit.end > view.state.doc.length) return
    view.dispatch({
      selection: { anchor: firstUnit.start, head: lastUnit.end }
    })
  }, [parseError, runGroup])

  useEffect(() => {
    const view = viewRef.current
    const databaseError = error?.databaseError
    if (!view || parseError || !error || !databaseError) return
    const editorSql = view.state.doc.toString()
    let selection = databaseErrorSelectionForEditor(
      editorSql,
      error.executedSql,
      databaseError
    )
    if (!selection) {
      const statement = runGroup?.run?.statements.find((candidate) =>
        candidate.sql === error.executedSql && candidate.result.kind === "error"
      )
      const unit = statement ? runGroup?.units[statement.statementIndex] : null
      if (unit && editorSql.slice(unit.start, unit.end) === error.executedSql) {
        const localSelection = databaseErrorSelection(error.executedSql, databaseError)
        if (localSelection) {
          selection = {
            from: unit.start + localSelection.from,
            to: unit.start + localSelection.to
          }
        }
      }
    }
    if (!selection) return
    view.dispatch({
      selection: { anchor: selection.from, head: selection.to },
      scrollIntoView: true
    })
  }, [error, parseError, runGroup])

  const [elapsedSample, setElapsedSample] = useState<{ queryRunId: string; value: number } | null>(null)
  const runningQueryRunId = runGroup?.owner.queryRunId ?? null
  const runningStartedAt = runGroup?.startedAt ?? null
  const runGroupStatus = runGroup?.status ?? null
  useEffect(() => {
    if (!running || !runningQueryRunId || runningStartedAt === null || runGroupStatus === "settled") {
      return
    }
    const timer = window.setInterval(() => {
      setElapsedSample({
        queryRunId: runningQueryRunId,
        value: Math.max(0, Math.round(performance.now() - runningStartedAt))
      })
    }, 100)
    return () => window.clearInterval(timer)
  }, [runGroupStatus, running, runningQueryRunId, runningStartedAt])
  const runningElapsedMs = running && runningQueryRunId
    ? elapsedSample?.queryRunId === runningQueryRunId
      ? elapsedSample.value
      : 0
    : null

  const canRun = connected && !needsDatabase && sqlText.trim().length > 0 && !running
  const canCancel = connected && queryRunGroupIsCancellable(runGroup)
  const runPrimary = () => {
    const view = viewRef.current
    if (!view) return
    const selection = view.state.selection.main
    void runQuery({
      kind: "primary",
      selection: { from: selection.from, to: selection.to },
      cursor: selection.head
    })
  }
  const activeProfileName = activeProfile?.name ?? activeConnection?.name ?? null
  const activeProfileKind = activeProfile?.kind ?? activeConnection?.kind ?? null
  const activeProfileAddress = activeProfile
    ? savedConnectionAddress(activeProfile)
    : (activeConnection?.title ?? null)
  const activeProfileEngine = activeProfileKind
    ? t(`databasePanel.engine.${activeProfileKind}`)
    : null

  const elapsedLabel = runningElapsedMs ?? elapsedMs
  const statementCount = runGroup?.run?.statements.length ?? 0

  const editable = !!table && table.kind === "table" && metadata.some((column) => column.pk)
  const resultMeta = (
    <>
      {(table || runGroup?.run) && (
        <span
          title={editable ? workbench("editHint") : workbench("readOnlyHint")}
          className="inline-flex shrink-0 items-center gap-1"
        >
          {editable
            ? <PencilLine className="size-3" aria-hidden="true" />
            : <Lock className="size-3" aria-hidden="true" />}
          {workbench(editable ? "editableShort" : "readOnlyShort")}
        </span>
      )}
      {statementCount > 1 && (
        <span className="shrink-0">{t("databasePanel.statementCount", { count: statementCount })}</span>
      )}
      {elapsedLabel != null && (
        <span className="shrink-0 font-mono tabular-nums">{t("databasePanel.elapsed", { ms: elapsedLabel })}</span>
      )}
    </>
  )

  return (
    <DatabaseCellEditing identity={identity} kind={activeConnection?.kind ?? "sqlite"} table={table} metadata={metadata} disabled={running || !!runGroup?.run?.transactionMayBeOpen}>
    <div className="database-console flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="database-profile-header flex shrink-0 items-center gap-2 border-b border-(--line-1) pl-4">
        <div
          role="group"
          aria-label={t("databasePanel.activeProfileAriaLabel", { name: activeProfileName })}
          className="flex min-w-0 flex-1 flex-col justify-center"
        >
          <div className="flex min-w-0 items-center gap-2 text-[13px] leading-5">
            <span
              aria-hidden="true"
              className={cn(
                "size-2 shrink-0 rounded-full",
                connected ? "bg-(--term-ok)" : "border border-(--ink-4)"
              )}
            />
            <span className="min-w-[3ch] shrink-[4] truncate font-medium text-(--ink-1)">{activeProfileName}</span>
            {connected
              ? <span className="sr-only">{t("databasePanel.connected")}</span>
              : <span className="shrink-0 text-[12px] text-(--ink-3)">{t("databasePanel.offline")}</span>}
            <span aria-hidden="true" className="shrink-0 text-(--ink-4)">/</span>
            <span className={cn("min-w-[6ch] truncate", table ? "font-mono text-[12.5px] text-(--ink-1)" : "text-(--ink-2)")}>
              {table?.name ?? workbench("query")}
            </span>
          </div>
          <p
            className="flex min-w-0 gap-2 font-mono text-[11px] leading-4 text-(--ink-3)"
            title={`${activeProfileEngine ?? ""} ${activeProfileAddress ?? ""}`}
          >
            <span className="shrink-0">{activeProfileEngine}</span>
            <span className="min-w-0 truncate">{activeProfileAddress}</span>
          </p>
        </div>
        <DatabaseCatalogPicker key={activeConnection?.connId ?? "offline"} descriptorId={descriptorId} />
        {table && (
          <ToggleGroup
            type="single"
            value={mode}
            onValueChange={(value) => { if (value) setModeChoice({ table, mode: value }) }}
            aria-label={workbench("viewMode")}
            className="shrink-0 gap-0.5 rounded-(--r-xs) bg-(--paper-2) p-0.5"
          >
            <ToggleGroupItem value="data" className="h-6 px-2.5 text-[12px] text-(--ink-3) hover:text-(--ink-1) data-[state=on]:bg-(--paper-0) data-[state=on]:text-(--ink-1) data-[state=on]:shadow-xs">
              {workbench("data")}
            </ToggleGroupItem>
            <ToggleGroupItem value="query" className="h-6 px-2.5 text-[12px] text-(--ink-3) hover:text-(--ink-1) data-[state=on]:bg-(--paper-0) data-[state=on]:text-(--ink-1) data-[state=on]:shadow-xs">
              {workbench("query")}
            </ToggleGroupItem>
          </ToggleGroup>
        )}
        {table && (
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={running}
            aria-label={workbench("refresh")}
            title={workbench("refresh")}
            onClick={() => void useDbStore.getState().openTableQuery(table)}
          >
            <RefreshCw />
          </Button>
        )}
      </div>
      {needsDatabase && <p role="status" className="shrink-0 border-b border-(--line-1) bg-(--amber-soft) px-4 py-2 text-[12px] text-(--ink-2)">{workbench("chooseDatabaseFirst")}</p>}
      {editUnconfirmed && <p role="status" className="shrink-0 border-b border-(--line-1) bg-(--amber-soft) px-4 py-2 text-[12px] text-(--ink-2)">{workbench("editAcceptedUnconfirmed")}</p>}
      <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
      <ResizablePanel id="database-query" panelRef={editorPanel} defaultSize="38%" minSize="15%" collapsible collapsedSize="0%">
      <section
        aria-label={t("databasePanel.queryCard")}
        className={cn("flex h-full min-h-0 flex-col", table && mode === "data" && "invisible")}
      >
        <div ref={containerRef} className="database-editor min-h-0 flex-1 overflow-hidden" />
        <div className="flex h-10 shrink-0 items-center gap-1 border-t border-(--line-1) bg-(--paper-1) px-2">
          <span className="min-w-0 flex-1 truncate pl-2 text-[11.5px] text-(--ink-3)">
            {running
              ? (runGroup?.status === "cancelling" ? t("databasePanel.cancelling") : t("databasePanel.running"))
              : null}
          </span>
          <Button
            type="button"
            onClick={() => void cancelQuery()}
            disabled={!canCancel}
            aria-label={t("databasePanel.cancelAriaLabel")}
            variant="ghost"
            size="sm"
          >
            <CircleStop aria-hidden="true" />
            {runGroup?.status === "cancelling"
              ? t("databasePanel.cancelling")
              : t("databasePanel.cancel")}
          </Button>
          <Button
            type="button"
            onClick={() => void runQuery({ kind: "all" })}
            disabled={!canRun}
            aria-label={t("databasePanel.runAllAriaLabel")}
            variant="ghost"
            size="sm"
          >
            {t("databasePanel.runAll")}
            <Kbd className="h-4 bg-(--paper-2) font-mono text-[10px] text-(--ink-3)">{shortcutLabel("mod-shift-enter")}</Kbd>
          </Button>
          <Button
            type="button"
            onClick={runPrimary}
            disabled={!canRun}
            aria-label={t("databasePanel.runAriaLabel")}
            size="sm"
          >
            <Play aria-hidden="true" />
            {t("databasePanel.run")}
            <Kbd className="h-4 bg-white/20 font-mono text-[10px] text-current">{shortcutLabel("mod-enter")}</Kbd>
          </Button>
        </div>
      </section>
      </ResizablePanel>
      <ResizableHandle
        aria-label={t("databasePanel.resizeResults")}
        disabled={mode === "data"}
        className="database-split-handle aria-disabled:pointer-events-none"
      />
      <ResizablePanel id="database-results" defaultSize="62%" minSize="25%">
      <section aria-label={t("databasePanel.resultCard")} className="flex h-full min-h-0 flex-col">
        <QueryRunView
          running={running}
          group={runGroup}
          parseError={parseError}
          result={result}
          error={error}
          sortBy={sortBy}
          meta={resultMeta}
          onSort={sortResult}
          onSelectStatement={selectStatementTab}
          onPreviousPage={previousResultPage}
          onNextPage={nextResultPage}
          onReleaseResult={releaseResultSession}
        />
      </section>
      </ResizablePanel>
      </ResizablePanelGroup>
    </div>
    </DatabaseCellEditing>
  )
}

function statementStatusKey(statement: DbStatementExecution): string {
  switch (statement.result.kind) {
    case "rows":
      return "rows"
    case "execute":
      return "execute"
    case "error":
      return "error"
    case "cancelled":
      return "cancelled"
    case "resultLimitReached":
      return "resultLimitReached"
    case "skipped":
      return "skipped"
  }
}

function ResultHeader({ children, meta }: { children: ReactNode; meta: ReactNode }) {
  return (
    <div className="flex h-9 shrink-0 items-stretch border-b border-(--line-1)">
      <div className="flex min-w-0 flex-1 items-stretch">{children}</div>
      <div className="flex shrink-0 items-center gap-3 pr-3 pl-2 text-[11.5px] text-(--ink-3)">{meta}</div>
    </div>
  )
}

function ResultStrip({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-(--line-1) bg-(--paper-1) px-3 py-1 text-[11.5px] text-(--ink-3)">
      {children}
    </div>
  )
}

function ResultPlaceholder({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full min-h-[96px] items-center justify-center px-6 text-center text-[12.5px] text-(--ink-3)">
      {children}
    </div>
  )
}

const QueryRunView = memo(function QueryRunView({
  running,
  group,
  parseError,
  result,
  error,
  sortBy,
  meta,
  onSort,
  onSelectStatement,
  onPreviousPage,
  onNextPage,
  onReleaseResult,
}: {
  running: boolean
  group: DbQueryRunGroup | null
  parseError: ReturnType<typeof queryFor>["parseError"]
  result: DbQueryResult | null
  error: DbQueryErrorState | null
  sortBy: DbSort | null
  meta: ReactNode
  onSort: (columnIndex: number, owner?: DbResultSessionOwner) => void
  onSelectStatement: (statementExecutionId: DbStatementExecution["statementExecutionId"]) => void
  onPreviousPage: (owner: DbResultSessionOwner) => Promise<void>
  onNextPage: (owner: DbResultSessionOwner) => Promise<void>
  onReleaseResult: (owner: DbResultSessionOwner) => Promise<void>
}) {
  const { t } = useTranslation("panels")
  const resultLabel = (
    <span className="flex items-center pl-4 text-[12px] font-medium text-(--ink-2)">{t("databasePanel.resultCard")}</span>
  )
  if (parseError) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <ResultHeader meta={meta}>{resultLabel}</ResultHeader>
        <ErrorBlock>{t(`databasePanel.parseError.${parseError.code}`)}</ErrorBlock>
      </div>
    )
  }
  if (!group?.run) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <ResultHeader meta={meta}>{resultLabel}</ResultHeader>
        <div className="min-h-0 flex-1">
          <ResultView
            running={running}
            result={result}
            error={error}
            sortBy={sortBy}
            onSort={onSort}
          />
        </div>
      </div>
    )
  }

  const run = group.run
  const active = run.statements.find((statement) =>
    statement.statementExecutionId === group.activeStatementExecutionId
  ) ?? run.statements[0]

  return (
    <Tabs
      value={active.statementExecutionId}
      onValueChange={(value) => onSelectStatement(value as DbStatementExecution["statementExecutionId"])}
      className="h-full min-h-0 gap-0"
    >
      <ResultHeader meta={meta}>
        <ScrollArea
          className="min-w-0 flex-1"
          orientation="horizontal"
          viewportClassName="[&>div]:h-full"
        >
          <TabsList
            variant="line"
            aria-label={t("databasePanel.statementTabsAriaLabel")}
            className="justify-start gap-0 p-0 pl-2 group-data-horizontal/tabs:h-9"
          >
            {run.statements.map((statement) => {
              const statusKey = statementStatusKey(statement)
              const status = t(`databasePanel.statementStatus.${statusKey}`)
              const failed = statusKey === "error" || statusKey === "cancelled"
              return (
                <TabsTrigger
                  key={statement.statementExecutionId}
                  value={statement.statementExecutionId}
                  aria-label={t("databasePanel.statementTabAriaLabel", {
                    index: statement.statementIndex + 1,
                    status,
                  })}
                  title={statement.sql}
                  className="flex-none self-start rounded-none px-2.5 text-[12px] font-normal text-(--ink-3) hover:text-(--ink-1) data-active:text-(--ink-1) group-data-horizontal/tabs:after:bottom-0"
                >
                  <span className="font-mono text-(--ink-4)">{statement.statementIndex + 1}</span>
                  <span className={cn(failed && "text-(--destructive)")}>{status}</span>
                </TabsTrigger>
              )
            })}
          </TabsList>
        </ScrollArea>
      </ResultHeader>
      {run.transactionMayBeOpen && (
        <div role="status" className="shrink-0 border-b border-(--line-1) bg-(--amber-soft) px-4 py-1.5 text-[12px] text-(--ink-2)">
          {t("databasePanel.transactionMayBeOpen")}
        </div>
      )}
      {(run.connectionTerminated || group.cancelOutcome === "cancelledConnectionTerminated") && (
        <div role="status" className="shrink-0 border-b border-(--line-1) bg-(--danger-soft) px-4 py-1.5 text-[12px] text-(--destructive)">
          {t("databasePanel.connectionTerminated")}
        </div>
      )}
      <TabsContent value={active.statementExecutionId} className="min-h-0 text-[length:inherit]">
        <StatementResult
          key={active.statementExecutionId}
          statement={active}
          pageState={resultPageStateForStatement(group, active)}
          onSort={onSort}
          onPreviousPage={onPreviousPage}
          onNextPage={onNextPage}
          onReleaseResult={onReleaseResult}
        />
      </TabsContent>
    </Tabs>
  )
})

function EffectOutcome({ outcome }: { outcome: DbEffectOutcome }) {
  const { t } = useTranslation("panels")
  if (outcome === "unknown") {
    return (
      <span className="inline-flex shrink-0 items-center gap-1 rounded-(--r-xs) bg-(--amber-soft) px-1.5 py-px font-medium text-(--ink-1)">
        <AlertTriangle className="size-3" aria-hidden="true" />
        {t(`databasePanel.effectOutcome.${outcome}`)}
      </span>
    )
  }
  return <span className="shrink-0">{t(`databasePanel.effectOutcome.${outcome}`)}</span>
}

function AffectedRows({ affectedRows }: { affectedRows: string | null }) {
  const { t } = useTranslation("panels")
  return (
    <div className="flex h-full min-h-[96px] items-center justify-center px-6 font-mono text-[13px] text-(--ink-1) tabular-nums">
      {affectedRows === null
        ? t("databasePanel.rowsAffectedUnavailable")
        : t(
            affectedRows === "1"
              ? "databasePanel.rowsAffected_one"
              : "databasePanel.rowsAffected_other",
            { value: affectedRows }
          )}
    </div>
  )
}

function StatementResult({
  statement,
  pageState,
  onSort,
  onPreviousPage,
  onNextPage,
  onReleaseResult,
}: {
  statement: DbStatementExecution
  pageState: DbStatementResultPageState | null
  onSort: (columnIndex: number, owner?: DbResultSessionOwner) => void
  onPreviousPage: (owner: DbResultSessionOwner) => Promise<void>
  onNextPage: (owner: DbResultSessionOwner) => Promise<void>
  onReleaseResult: (owner: DbResultSessionOwner) => Promise<void>
}) {
  const { t } = useTranslation("panels")
  const result = statement.result
  const resultSession = result.kind === "rows"
    ? result.resultSession
    : result.kind === "resultLimitReached"
      ? result.resultSession
      : null
  const outcome = pageState?.page.effectOutcome ?? statement.effectOutcome
  if ((result.kind === "rows" || result.kind === "resultLimitReached") && resultSession && pageState) {
    return (
      <ResultSessionPage
        owner={resultSession.owner}
        state={pageState}
        outcome={outcome}
        sortBy={pageState.sort}
        onSort={(columnIndex) => onSort(columnIndex, resultSession.owner)}
        onPreviousPage={onPreviousPage}
        onNextPage={onNextPage}
        onReleaseResult={onReleaseResult}
      />
    )
  }
  let content: ReactNode
  if (result.kind === "rows" || result.kind === "resultLimitReached") {
    content = <ResultPlaceholder>{t("databasePanel.noResultSession")}</ResultPlaceholder>
  } else if (result.kind === "execute") {
    content = <AffectedRows affectedRows={result.affectedRows} />
  } else if (result.kind === "error" || result.kind === "cancelled") {
    content = <DatabaseErrorDetails error={result.error} />
  } else {
    content = <ResultPlaceholder>{t(`databasePanel.statementStatus.${result.kind}`)}</ResultPlaceholder>
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScrollArea className="min-h-0 flex-1">{content}</ScrollArea>
      <ResultStrip><EffectOutcome outcome={outcome} /></ResultStrip>
    </div>
  )
}

function ResultSessionPage({
  owner,
  state,
  outcome,
  sortBy,
  onSort,
  onPreviousPage,
  onNextPage,
  onReleaseResult,
}: {
  owner: DbResultSessionOwner
  state: DbStatementResultPageState
  outcome: DbEffectOutcome
  sortBy: DbSort | null
  onSort: (columnIndex: number) => void
  onPreviousPage: (owner: DbResultSessionOwner) => Promise<void>
  onNextPage: (owner: DbResultSessionOwner) => Promise<void>
  onReleaseResult: (owner: DbResultSessionOwner) => Promise<void>
}) {
  const { t } = useTranslation("panels")
  const page = state.page
  const showEnd = !page.hasNext &&
    !page.resultLimitReached &&
    page.lifecycle !== "released" &&
    page.lifecycle !== "cancelled" &&
    page.lifecycle !== "error"

  const footer = (
    <>
      <EffectOutcome outcome={outcome} />
      {state.loading && <span role="status">{t("databasePanel.loadingResultPage")}</span>}
      {showEnd && <span role="status">{t("databasePanel.resultEnd")}</span>}
      {page.resultLimitReached && (
        <span role="status" className="text-(--amber)">
          {t("databasePanel.resultLimitReached")}
        </span>
      )}
      {page.lifecycle === "released" && (
        <span role="status">{t("databasePanel.resultReleased")}</span>
      )}
      {page.lifecycle === "cancelled" && (
        <span role="status">{t("databasePanel.resultCancelled")}</span>
      )}
      {page.lifecycle === "error" && (
        <span role="status" className="text-(--destructive)">
          {t("databasePanel.resultLifecycleError")}
        </span>
      )}
      <div className="ml-auto flex shrink-0 items-center gap-0.5">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={page.lifecycle !== "streaming" || state.loading}
          aria-label={t("databasePanel.releaseResultAriaLabel")}
          onClick={() => void onReleaseResult(owner)}
          className="mr-1 text-[11.5px] text-(--ink-2)"
        >
          {t("databasePanel.releaseResult")}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          disabled={!page.hasPrevious || state.loading}
          aria-label={t("databasePanel.previousPageAriaLabel")}
          title={t("databasePanel.previousPage")}
          onClick={() => void onPreviousPage(owner)}
        >
          <ChevronLeft aria-hidden="true" />
        </Button>
        <span className="min-w-12 text-center font-mono text-(--ink-2) tabular-nums">
          {t("databasePanel.pageLabel", { page: page.pageIndex + 1 })}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          disabled={
            !page.hasNext ||
            page.lifecycle === "released" ||
            page.lifecycle === "cancelled" ||
            page.lifecycle === "error" ||
            state.loading
          }
          aria-label={t("databasePanel.nextPageAriaLabel")}
          title={t("databasePanel.nextPage")}
          onClick={() => void onNextPage(owner)}
        >
          <ChevronRight aria-hidden="true" />
        </Button>
      </div>
    </>
  )

  return (
    <div className="flex h-full min-h-0 flex-col">
      {state.pageError && (
        <div role="alert" className="shrink-0 border-b border-(--line-1) bg-(--danger-soft) px-4 py-1.5 text-[12px] text-(--destructive)">
          {state.pageError.databaseError?.message ?? t("databasePanel.resultPageError")}
        </div>
      )}
      <div className="min-h-0 flex-1">
        <ResultTable
          columns={page.columns}
          rows={page.rows}
          truncated={page.resultLimitReached}
          sortBy={sortBy}
          onSort={onSort}
          footer={footer}
          resetKey={page.pageIndex}
        />
      </div>
    </div>
  )
}

function ErrorBlock({ children, mono = false }: { children: ReactNode; mono?: boolean }) {
  return (
    <div
      role="alert"
      className={cn(
        "m-3 rounded-(--r-xs) border border-(--destructive)/30 bg-(--danger-soft) px-3 py-2 text-[12px] whitespace-pre-wrap text-(--destructive)",
        mono && "font-mono"
      )}
    >
      {children}
    </div>
  )
}

function DatabaseErrorDetails({ error }: { error: DbError }) {
  return (
    <ErrorBlock mono>
      <div>{error.message}</div>
      {error.detail && <div className="mt-1 text-(--ink-2)">{error.detail}</div>}
      {error.hint && <div className="mt-1 text-(--ink-2)">{error.hint}</div>}
    </ErrorBlock>
  )
}

function ResultView({
  running,
  result,
  error,
  sortBy,
  onSort
}: {
  running: boolean
  result: DbQueryResult | null
  error: DbQueryErrorState | null
  sortBy: DbSort | null
  onSort: (columnIndex: number) => void
}) {
  const { t } = useTranslation("panels")
  const { t: tWorkbench } = useTranslation("workbench")
  if (error) {
    return (
      <ErrorBlock mono>
        {error.databaseError ? (
          <>
            <div>{error.databaseError.message}</div>
            {error.databaseError.detail && <div className="mt-1 text-(--ink-2)">{error.databaseError.detail}</div>}
            {error.databaseError.hint && <div className="mt-1 text-(--ink-2)">{error.databaseError.hint}</div>}
          </>
        ) : (
          tWorkbench(`database.profileError.${error.code}`)
        )}
      </ErrorBlock>
    )
  }
  // A running query with a prior result keeps the existing table mounted (rather
  // than swapping in a "Running…" placeholder) so a header sort re-run doesn't
  // unmount ResultTable and lose the user's dragged column order.
  if (!result) {
    if (running) {
      return <ResultPlaceholder><span role="status">{t("databasePanel.running")}</span></ResultPlaceholder>
    }
    return (
      <ResultPlaceholder>
        <span className="flex flex-col items-center gap-2">
          <span>{t("databasePanel.runPrompt")}</span>
          <span className="flex items-center gap-1.5 text-[11.5px] text-(--ink-4)">
            <Kbd className="font-mono text-[10.5px]">{shortcutLabel("mod-enter")}</Kbd>
            {t("databasePanel.runPromptShortcut")}
          </span>
        </span>
      </ResultPlaceholder>
    )
  }
  if (result.kind === "execute") {
    return <AffectedRows affectedRows={result.affectedRows} />
  }
  return (
    <ResultTable
      columns={result.columns}
      rows={result.rows}
      truncated={result.truncated}
      sortBy={sortBy}
      onSort={onSort}
    />
  )
}

function isNumericValue(value: DbValue | undefined): boolean {
  return value?.kind === "integer" || value?.kind === "decimal"
}

const ResultTable = memo(function ResultTable({
  columns,
  rows,
  truncated,
  sortBy,
  onSort,
  footer,
  resetKey
}: {
  columns: string[]
  rows: DbValue[][]
  truncated: boolean
  sortBy: DbSort | null
  onSort: (columnIndex: number) => void
  footer?: ReactNode
  /** Changing it (e.g. a new result page) scrolls back to the first row. */
  resetKey?: number
}) {
  const { t } = useTranslation("panels")
  const { t: workbench } = useTranslation("databaseWorkbench")
  const editing = useDatabaseCellEditing()
  const viewport = useRef<HTMLDivElement>(null)
  const [window, setWindow] = useState({ top: 0, height: 600 })
  useEffect(() => {
    const element = viewport.current
    if (!element) return
    const observer = new ResizeObserver(() => setWindow(current => {
      const height = element.clientHeight || 600
      return current.height === height ? current : { ...current, height }
    }))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  const [previousResetKey, setPreviousResetKey] = useState(resetKey)
  if (previousResetKey !== resetKey) {
    setPreviousResetKey(resetKey)
    setWindow(current => ({ ...current, top: 0 }))
  }
  useEffect(() => {
    if (viewport.current) viewport.current.scrollTop = 0
  }, [resetKey])
  const rowHeight = 29
  const start = Math.min(Math.max(0, rows.length - 1), Math.max(0, Math.floor((window.top - 32) / rowHeight) - 8))
  const end = Math.min(rows.length, start + Math.ceil(window.height / rowHeight) + 16)
  // Display order → original column index. Resets when the column *values*
  // change (a genuinely different query) but not when a header sort re-runs and
  // returns a new array with the same names.
  const [order, setOrder] = useState<number[]>(() => columns.map((_, i) => i))
  const [previousColumns, setPreviousColumns] = useState(columns)
  if (previousColumns !== columns) {
    const changed = !sameColumns(previousColumns, columns)
    setPreviousColumns(columns)
    if (changed) setOrder(columns.map((_, i) => i))
  }
  // A changed result can render once before React applies the order reset above.
  // Never index the new row shape through stale original-column positions.
  const displayOrder = order.length === columns.length && order.every((index) => index < columns.length)
    ? order
    : columns.map((_, index) => index)
  const [dragPos, setDragPos] = useState<number | null>(null)
  // Right-align a column when its first non-NULL sampled value is numeric.
  const numericColumns = useMemo(() => columns.map((_, columnIndex) => {
    for (let rowIndex = 0; rowIndex < Math.min(rows.length, 50); rowIndex += 1) {
      const value = rows[rowIndex][columnIndex]
      if (value && value.kind !== "null") return isNumericValue(value)
    }
    return false
  }), [columns, rows])
  const spanWithGutter = columns.length + 1
  const gutterWidth = `${Math.max(2, String(rows.length).length) + 2}ch`

  return (
    <div className="flex h-full flex-col">
      <ScrollArea className="min-h-0 flex-1" orientation="both" viewportRef={viewport} viewportProps={{ onScroll: event => {
        const top = event.currentTarget.scrollTop
        setWindow(current => Math.floor(current.top / rowHeight) === Math.floor(top / rowHeight) ? current : { ...current, top })
      } }}>
        <Table aria-rowcount={rows.length + 1} className="w-max min-w-full border-separate border-spacing-0 font-mono text-[12px]">
          <TableHeader className="sticky top-0 z-10 bg-(--paper-0) [&_tr]:border-0">
            <TableRow className="hover:bg-transparent">
              <th
                aria-hidden="true"
                style={{ width: gutterWidth, minWidth: gutterWidth }}
                className="sticky left-0 z-10 border-r border-b border-(--line-1) bg-(--paper-1)"
              />
              {displayOrder.map((origIdx, pos) => {
                const active = sortBy?.columnIndex === origIdx
                return (
                  <TableHead
                    key={origIdx}
                    aria-sort={
                      active ? (sortBy!.dir === "asc" ? "ascending" : "descending") : "none"
                    }
                    className={cn(
                      "h-8 border-r border-b border-(--line-1) p-0 text-(--ink-2) last:border-r-0",
                      dragPos === pos && "bg-(--yz-hover)"
                    )}
                  >
                    <Button
                      type="button"
                      variant="ghost"
                      draggable
                      onClick={() => onSort(origIdx)}
                      onDragStart={() => setDragPos(pos)}
                      onDragOver={(e) => e.preventDefault()}
                      onDrop={(e) => {
                        e.preventDefault()
                        if (dragPos !== null && dragPos !== pos) {
                          setOrder((o) => reorderColumns(o, dragPos, pos))
                        }
                        setDragPos(null)
                      }}
                      onDragEnd={() => setDragPos(null)}
                      aria-label={t("databasePanel.sortColumn", { column: columns[origIdx] })}
                      className={cn(
                        "w-full gap-1 rounded-none px-3 text-[12px] hover:text-(--ink-1)",
                        numericColumns[origIdx] ? "justify-end text-right" : "justify-start text-left",
                        active && "text-(--ink-1)"
                      )}
                    >
                      <span>{columns[origIdx]}</span>
                      {active &&
                        (sortBy!.dir === "asc" ? (
                          <ChevronUp className="size-3 shrink-0" aria-hidden="true" />
                        ) : (
                          <ChevronDown className="size-3 shrink-0" aria-hidden="true" />
                        ))}
                    </Button>
                  </TableHead>
                )
              })}
            </TableRow>
          </TableHeader>
          <TableBody>
            {start > 0 && <TableRow aria-hidden="true" className="border-0 hover:bg-transparent"><TableCell colSpan={spanWithGutter} style={{ height: start * rowHeight, padding: 0 }} /></TableRow>}
            {rows.slice(start, end).map((row, ri) => (
              <TableRow key={start + ri} aria-rowindex={start + ri + 2} className="group/row border-0 hover:bg-(--yz-hover)" style={{ height: rowHeight }}>
                <td
                  aria-hidden="true"
                  className="sticky left-0 border-r border-b border-(--line-1) bg-(--paper-1) px-2 text-right text-[11px] text-(--ink-4) tabular-nums group-hover/row:text-(--ink-2)"
                >
                  {start + ri + 1}
                </td>
                {displayOrder.map((origIdx) => {
                  const v = row[origIdx]
                  const display = formatDbValue(v)
                  const cellEditable = editing?.editable(columns[origIdx], v) ?? false
                  return (
                    <TableCell
                      key={origIdx}
                      tabIndex={cellEditable ? 0 : undefined}
                      onDoubleClick={() => editing?.edit(columns, row, columns[origIdx])}
                      onKeyDown={event => { if (editing && (event.key === "Enter" || event.key === "F2")) { event.preventDefault(); editing.edit(columns, row, columns[origIdx]) } }}
                      title={cellEditable ? workbench("editHint") : workbench("readOnlyHint")}
                      className={cn(
                        "border-r border-b border-(--line-1)/70 px-3 py-0 whitespace-nowrap text-(--ink-1) outline-none last:border-r-0 focus-visible:bg-(--yz-active) focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]",
                        numericColumns[origIdx] && "text-right tabular-nums",
                        cellEditable && "cursor-text"
                      )}
                    >
                      <span className={cn("block max-w-[360px] truncate", numericColumns[origIdx] && "ml-auto")} title={display ?? "NULL"}>
                      {display === null ? (
                        <span className="text-(--ink-4) italic">NULL</span>
                      ) : (
                        display
                      )}
                      </span>
                    </TableCell>
                  )
                })}
              </TableRow>
            ))}
            {end < rows.length && <TableRow aria-hidden="true" className="border-0 hover:bg-transparent"><TableCell colSpan={spanWithGutter} style={{ height: (rows.length - end) * rowHeight, padding: 0 }} /></TableRow>}
            {rows.length === 0 && (
              <TableRow className="border-0 hover:bg-transparent">
                <TableCell
                  colSpan={spanWithGutter}
                  className="px-4 py-6 text-center font-sans text-[12.5px] text-(--ink-3)"
                >
                  {t("databasePanel.noRows")}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </ScrollArea>
      <ResultStrip>
        <span className="shrink-0 font-mono text-(--ink-2) tabular-nums">
          {t("databasePanel.rowCount", { count: rows.length })}
        </span>
        {truncated && (
          <span className="shrink-0 rounded-(--r-xs) bg-(--amber-soft) px-1.5 py-px text-(--ink-1)">
            {t("databasePanel.truncated", { count: rows.length })}
          </span>
        )}
        {footer}
      </ResultStrip>
    </div>
  )
})
