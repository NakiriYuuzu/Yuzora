import { useEffect, useRef, useState } from "react"
import type { ReactNode } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView, keymap, lineNumbers } from "@codemirror/view"
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands"
import { syntaxHighlighting } from "@codemirror/language"
import { sql } from "@codemirror/lang-sql"
import { ChevronDown, ChevronUp, ListStart, Play, Square, Table2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { databaseErrorSelection, databaseErrorSelectionForEditor } from "@/lib/databaseSql"
import {
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
import { formatDbValue } from "@/lib/types"
import type {
  DbEffectOutcome,
  DbError,
  DbQueryResult,
  DbResultSessionOwner,
  DbStatementExecution,
  DbValue,
} from "@/lib/types"

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
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
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
  const containerRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const activeProfile = useDbStore((state) =>
    state.saved.find((profile) => profile.id === descriptorId) ?? null
  )
  const activeConnection = useDbStore((state) =>
    state.connections.find((connection) => connection.descriptorId === descriptorId) ?? null
  )
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

  const canRun = connected && sqlText.trim().length > 0 && !running
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

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-col">
        {activeProfileName && activeProfileAddress && activeProfileEngine && (
          <div
            role="group"
            aria-label={t("databasePanel.activeProfileAriaLabel", { name: activeProfileName })}
            className="flex h-[32px] shrink-0 items-center gap-[8px] border-b border-(--line-1) px-[10px]"
          >
            <span className="shrink-0 text-[10px] font-semibold tracking-[0.06em] text-(--ink-4) uppercase">
              {t("databasePanel.activeProfile")}
            </span>
            <span className="min-w-0 truncate text-[12px] font-medium text-(--ink-1)">
              {activeProfileName}
            </span>
            <span
              className="ml-auto min-w-0 truncate font-mono text-[10.5px] text-(--ink-3)"
              title={`${activeProfileEngine} · ${activeProfileAddress}`}
            >
              {activeProfileEngine} · {activeProfileAddress}
            </span>
          </div>
        )}
        <div ref={containerRef} className="h-[160px] overflow-auto" />
        <div className="flex h-[40px] shrink-0 items-center gap-[8px] border-t border-b border-(--line-1) px-[10px]">
          <button
            type="button"
            onClick={runPrimary}
            disabled={!canRun}
            aria-label={t("databasePanel.runAriaLabel")}
            className="flex h-[26px] items-center gap-[6px] rounded-[8px] bg-(--yz-accent) px-[10px] text-[12px] font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
          >
            <Play className="size-[13px]" aria-hidden="true" />
            {t("databasePanel.run")}
            <kbd className="rounded-[5px] bg-white/20 px-[5px] py-[1px] font-mono text-[10px]">⌘↵</kbd>
          </button>
          <button
            type="button"
            onClick={() => void runQuery({ kind: "all" })}
            disabled={!canRun}
            aria-label={t("databasePanel.runAllAriaLabel")}
            className="flex h-[26px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[10px] text-[12px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover) disabled:opacity-50"
          >
            <ListStart className="size-[13px]" aria-hidden="true" />
            {t("databasePanel.runAll")}
            <kbd className="rounded-[5px] bg-(--paper-2) px-[5px] py-[1px] font-mono text-[10px]">⇧⌘↵</kbd>
          </button>
          <button
            type="button"
            onClick={() => void cancelQuery()}
            disabled={!canCancel}
            aria-label={t("databasePanel.cancelAriaLabel")}
            className="flex h-[26px] items-center gap-[6px] rounded-[8px] border border-(--destructive)/30 px-[10px] text-[12px] font-medium text-(--destructive) transition-colors hover:bg-(--danger-soft) disabled:opacity-40"
          >
            <Square className="size-[12px]" aria-hidden="true" />
            {runGroup?.status === "cancelling"
              ? t("databasePanel.cancelling")
              : t("databasePanel.cancel")}
          </button>
          <div className="flex-1" />
          {(runningElapsedMs ?? elapsedMs) != null && (
            <span className="font-mono text-[11px] text-(--ink-3)">
              {runningElapsedMs ?? elapsedMs} ms
            </span>
          )}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <QueryRunView
          running={running}
          group={runGroup}
          parseError={parseError}
          result={result}
          error={error}
          sortBy={sortBy}
          onSort={sortResult}
          onSelectStatement={selectStatementTab}
          onPreviousPage={previousResultPage}
          onNextPage={nextResultPage}
          onReleaseResult={releaseResultSession}
        />
      </div>
    </div>
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

function QueryRunView({
  running,
  group,
  parseError,
  result,
  error,
  sortBy,
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
  onSort: (columnIndex: number, owner?: DbResultSessionOwner) => void
  onSelectStatement: (statementExecutionId: DbStatementExecution["statementExecutionId"]) => void
  onPreviousPage: (owner: DbResultSessionOwner) => Promise<void>
  onNextPage: (owner: DbResultSessionOwner) => Promise<void>
  onReleaseResult: (owner: DbResultSessionOwner) => Promise<void>
}) {
  const { t } = useTranslation("panels")
  if (parseError) {
    return (
      <div role="alert" className="m-[10px] rounded-[8px] border border-(--destructive)/40 bg-(--danger-soft) px-[10px] py-[8px] text-[12px] text-(--destructive)">
        {t(`databasePanel.parseError.${parseError.code}`)}
      </div>
    )
  }
  if (!group?.run) {
    return (
      <ResultView
        running={running}
        result={result}
        error={error}
        sortBy={sortBy}
        onSort={onSort}
      />
    )
  }

  const run = group.run
  const active = run.statements.find((statement) =>
    statement.statementExecutionId === group.activeStatementExecutionId
  ) ?? run.statements[0]

  return (
    <div className="flex h-full min-h-0 flex-col">
      {run.transactionMayBeOpen && (
        <div role="status" className="border-b border-(--amber)/30 bg-(--amber-soft) px-[10px] py-[6px] text-[11px] text-(--ink-2)">
          {t("databasePanel.transactionMayBeOpen")}
        </div>
      )}
      {(run.connectionTerminated || group.cancelOutcome === "cancelledConnectionTerminated") && (
        <div role="status" className="border-b border-(--destructive)/30 bg-(--danger-soft) px-[10px] py-[6px] text-[11px] text-(--destructive)">
          {t("databasePanel.connectionTerminated")}
        </div>
      )}
      <div
        role="tablist"
        aria-label={t("databasePanel.statementTabsAriaLabel")}
        className="flex shrink-0 gap-[2px] overflow-x-auto border-b border-(--line-1) bg-(--paper-1) px-[6px] pt-[5px]"
      >
        {run.statements.map((statement, statementPosition) => {
          const status = t(`databasePanel.statementStatus.${statementStatusKey(statement)}`)
          const selected = statement.statementExecutionId === active.statementExecutionId
          return (
            <button
              key={statement.statementExecutionId}
              id={`db-statement-tab-${statement.statementExecutionId}`}
              type="button"
              role="tab"
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              aria-controls={`db-statement-panel-${statement.statementExecutionId}`}
              aria-label={t("databasePanel.statementTabAriaLabel", {
                index: statement.statementIndex + 1,
                status,
              })}
              title={statement.sql}
              onClick={() => onSelectStatement(statement.statementExecutionId)}
              onKeyDown={(event) => {
                let nextPosition: number | null = null
                if (event.key === "ArrowRight") {
                  nextPosition = (statementPosition + 1) % run.statements.length
                } else if (event.key === "ArrowLeft") {
                  nextPosition = (statementPosition - 1 + run.statements.length) % run.statements.length
                } else if (event.key === "Home") {
                  nextPosition = 0
                } else if (event.key === "End") {
                  nextPosition = run.statements.length - 1
                }
                if (nextPosition === null) return
                event.preventDefault()
                const tabs = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
                  '[role="tab"]'
                )
                tabs?.[nextPosition]?.focus()
                onSelectStatement(run.statements[nextPosition].statementExecutionId)
              }}
              className={`rounded-t-[7px] border border-b-0 px-[9px] py-[5px] text-[11px] ${
                selected
                  ? "border-(--line-1) bg-(--paper-0) text-(--ink-1)"
                  : "border-transparent text-(--ink-3) hover:text-(--ink-1)"
              }`}
            >
              <span className="font-mono">{statement.statementIndex + 1}</span>
              <span className="ml-[5px]">{status}</span>
            </button>
          )
        })}
      </div>
      <div
        id={`db-statement-panel-${active.statementExecutionId}`}
        role="tabpanel"
        aria-labelledby={`db-statement-tab-${active.statementExecutionId}`}
        className="min-h-0 flex-1"
      >
        <StatementResult
          key={active.statementExecutionId}
          statement={active}
          pageState={resultPageStateForStatement(group, active)}
          onSort={onSort}
          onPreviousPage={onPreviousPage}
          onNextPage={onNextPage}
          onReleaseResult={onReleaseResult}
        />
      </div>
    </div>
  )
}

function EffectOutcome({ outcome }: { outcome: DbEffectOutcome }) {
  const { t } = useTranslation("panels")
  return (
    <div
      className={`shrink-0 border-b border-(--line-1) px-[10px] py-[5px] text-[11px] ${
        outcome === "unknown" ? "bg-(--amber-soft) font-semibold text-(--ink-1)" : "text-(--ink-3)"
      }`}
    >
      {t(`databasePanel.effectOutcome.${outcome}`)}
    </div>
  )
}

function AffectedRows({ affectedRows }: { affectedRows: string | null }) {
  const { t } = useTranslation("panels")
  return (
    <div className="p-[12px] font-mono text-[12px] text-(--ink-2)">
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
  let content: ReactNode
  if (result.kind === "rows" || result.kind === "resultLimitReached") {
    content = resultSession && pageState ? (
      <ResultSessionPage
        owner={resultSession.owner}
        state={pageState}
        sortBy={pageState.sort}
        onSort={(columnIndex) => onSort(columnIndex, resultSession.owner)}
        onPreviousPage={onPreviousPage}
        onNextPage={onNextPage}
        onReleaseResult={onReleaseResult}
      />
    ) : (
      <div className="p-[12px] text-[12px] text-(--ink-3)">{t("databasePanel.noResultSession")}</div>
    )
  } else if (result.kind === "execute") {
    content = <AffectedRows affectedRows={result.affectedRows} />
  } else if (result.kind === "error" || result.kind === "cancelled") {
    content = <DatabaseErrorDetails error={result.error} />
  } else {
    content = (
      <div className="p-[12px] text-[12px] text-(--ink-3)">
        {t(`databasePanel.statementStatus.${result.kind}`)}
      </div>
    )
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <EffectOutcome outcome={pageState?.page.effectOutcome ?? statement.effectOutcome} />
      <div className="min-h-0 flex-1">{content}</div>
    </div>
  )
}

function ResultSessionPage({
  owner,
  state,
  sortBy,
  onSort,
  onPreviousPage,
  onNextPage,
  onReleaseResult,
}: {
  owner: DbResultSessionOwner
  state: DbStatementResultPageState
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

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-[6px] border-b border-(--line-1) px-[10px] py-[6px] text-[11px] text-(--ink-3)">
        <span className="mr-[2px] font-mono font-semibold text-(--ink-2)">
          {t("databasePanel.pageLabel", { page: page.pageIndex + 1 })}
        </span>
        <button
          type="button"
          disabled={!page.hasPrevious || state.loading}
          aria-label={t("databasePanel.previousPageAriaLabel")}
          onClick={() => void onPreviousPage(owner)}
          className="rounded-[6px] border border-(--line-1) px-[7px] py-[3px] text-(--ink-2) hover:bg-(--yz-hover) disabled:opacity-40"
        >
          {t("databasePanel.previousPage")}
        </button>
        <button
          type="button"
          disabled={
            !page.hasNext ||
            page.lifecycle === "released" ||
            page.lifecycle === "cancelled" ||
            page.lifecycle === "error" ||
            state.loading
          }
          aria-label={t("databasePanel.nextPageAriaLabel")}
          onClick={() => void onNextPage(owner)}
          className="rounded-[6px] border border-(--line-1) px-[7px] py-[3px] text-(--ink-2) hover:bg-(--yz-hover) disabled:opacity-40"
        >
          {t("databasePanel.nextPage")}
        </button>
        <button
          type="button"
          disabled={page.lifecycle !== "streaming" || state.loading}
          aria-label={t("databasePanel.releaseResultAriaLabel")}
          onClick={() => void onReleaseResult(owner)}
          className="rounded-[6px] border border-(--line-1) px-[7px] py-[3px] text-(--ink-2) hover:bg-(--yz-hover) disabled:opacity-40"
        >
          {t("databasePanel.releaseResult")}
        </button>
        <div className="ml-auto flex flex-wrap items-center gap-[6px]">
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
        </div>
      </div>
      {state.pageError && (
        <div role="alert" className="border-b border-(--destructive)/30 bg-(--danger-soft) px-[10px] py-[6px] text-[11px] text-(--destructive)">
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
        />
      </div>
    </div>
  )
}

function DatabaseErrorDetails({ error }: { error: DbError }) {
  return (
    <div role="alert" className="m-[10px] rounded-[8px] border border-(--destructive)/40 bg-(--danger-soft) px-[10px] py-[8px] font-mono text-[12px] whitespace-pre-wrap text-(--destructive)">
      <div>{error.message}</div>
      {error.detail && <div>{error.detail}</div>}
      {error.hint && <div>{error.hint}</div>}
    </div>
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
      <div role="alert" className="m-[10px] rounded-[8px] border border-(--destructive)/40 bg-(--danger-soft) px-[10px] py-[8px] font-mono text-[12px] whitespace-pre-wrap text-(--destructive)">
        {error.databaseError ? (
          <>
            <div>{error.databaseError.message}</div>
            {error.databaseError.detail && <div>{error.databaseError.detail}</div>}
            {error.databaseError.hint && <div>{error.databaseError.hint}</div>}
          </>
        ) : (
          tWorkbench(`database.profileError.${error.code}`)
        )}
      </div>
    )
  }
  // A running query with a prior result keeps the existing table mounted (rather
  // than swapping in a "Running…" placeholder) so a header sort re-run doesn't
  // unmount ResultTable and lose the user's dragged column order.
  if (!result) {
    if (running) {
      return <div className="p-[12px] text-[12px] text-(--ink-3)">{t("databasePanel.running")}</div>
    }
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-[12px] text-(--ink-4)">{t("databasePanel.runPrompt")}</p>
      </div>
    )
  }
  if (result.kind === "execute") {
    return (
      <div className="p-[12px] font-mono text-[12px] text-(--ink-2)">
        {result.affectedRows === null
          ? t("databasePanel.rowsAffectedUnavailable")
          : t(
              result.affectedRows === "1"
                ? "databasePanel.rowsAffected_one"
                : "databasePanel.rowsAffected_other",
              { value: result.affectedRows }
            )}
      </div>
    )
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

function ResultTable({
  columns,
  rows,
  truncated,
  sortBy,
  onSort
}: {
  columns: string[]
  rows: DbValue[][]
  truncated: boolean
  sortBy: DbSort | null
  onSort: (columnIndex: number) => void
}) {
  const { t } = useTranslation("panels")
  // Display order → original column index. Resets when the column *values*
  // change (a genuinely different query) but not when a header sort re-runs and
  // returns a new array with the same names.
  const [order, setOrder] = useState<number[]>(() => columns.map((_, i) => i))
  const prevColumnsRef = useRef(columns)
  if (prevColumnsRef.current !== columns) {
    const changed = !sameColumns(prevColumnsRef.current, columns)
    prevColumnsRef.current = columns
    if (changed) setOrder(columns.map((_, i) => i))
  }
  // A changed result can render once before React applies the order reset above.
  // Never index the new row shape through stale original-column positions.
  const displayOrder = order.length === columns.length && order.every((index) => index < columns.length)
    ? order
    : columns.map((_, index) => index)
  const [dragPos, setDragPos] = useState<number | null>(null)

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse font-mono text-[12px]">
          <thead className="sticky top-0 bg-(--paper-1)">
            <tr>
              {displayOrder.map((origIdx, pos) => {
                const active = sortBy?.columnIndex === origIdx
                return (
                  <th
                    key={origIdx}
                    aria-sort={
                      active ? (sortBy!.dir === "asc" ? "ascending" : "descending") : "none"
                    }
                    className="border-b border-r border-(--line-1)/60 p-0 text-(--ink-2) last:border-r-0"
                  >
                    <button
                      type="button"
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
                      className="flex w-full cursor-pointer items-center gap-[4px] px-[10px] py-[5px] text-left font-semibold whitespace-nowrap select-none hover:text-(--ink-1)"
                    >
                      <span>{columns[origIdx]}</span>
                      {active &&
                        (sortBy!.dir === "asc" ? (
                          <ChevronUp className="size-[12px] shrink-0" aria-hidden="true" />
                        ) : (
                          <ChevronDown className="size-[12px] shrink-0" aria-hidden="true" />
                        ))}
                    </button>
                  </th>
                )
              })}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, ri) => (
              <tr key={ri} className="hover:bg-(--yz-hover)">
                {displayOrder.map((origIdx) => {
                  const v = row[origIdx]
                  const display = formatDbValue(v)
                  return (
                    <td
                      key={origIdx}
                      className="border-r border-b border-(--line-1)/60 px-[10px] py-[4px] whitespace-nowrap text-(--ink-1) last:border-r-0"
                    >
                      {display === null ? (
                        <span className="text-(--ink-4) italic">NULL</span>
                      ) : (
                        display
                      )}
                    </td>
                  )
                })}
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td
                  colSpan={Math.max(1, columns.length)}
                  className="px-[10px] py-[8px] text-[12px] text-(--ink-4)"
                >
                  {t("databasePanel.noRows")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 items-center gap-[8px] border-t border-(--line-1) px-[10px] py-[5px] text-[11px] text-(--ink-3)">
        <span className="font-mono">
          {t("databasePanel.rowCount", { count: rows.length })}
        </span>
        {truncated && (
          <span
            className="rounded-(--r-pill) bg-(--amber-soft) px-[6px] py-[1px] font-mono text-[10px]"
            style={{ color: "#9a6512" }}
          >
            {t("databasePanel.truncated", { count: rows.length })}
          </span>
        )}
      </div>
    </div>
  )
}
