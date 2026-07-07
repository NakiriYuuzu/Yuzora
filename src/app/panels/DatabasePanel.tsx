import { useEffect, useRef, useState } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView, keymap, lineNumbers } from "@codemirror/view"
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands"
import { syntaxHighlighting } from "@codemirror/language"
import { sql } from "@codemirror/lang-sql"
import { ChevronDown, ChevronUp, Play, Table2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { queryFor, useDbStore } from "@/state/dbStore"
import type { DbSort } from "@/state/dbStore"
import type { DbQueryResult, DbValue } from "@/lib/types"

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
  const hasConnection = useDbStore((s) => s.activeConnId !== null)

  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      {hasConnection ? (
        <DatabaseConsole />
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

function DatabaseConsole() {
  const { t } = useTranslation("panels")
  const containerRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const sqlText = useDbStore((s) => queryFor(s, s.activeConnId).sql)
  const running = useDbStore((s) => queryFor(s, s.activeConnId).running)
  const result = useDbStore((s) => queryFor(s, s.activeConnId).result)
  const error = useDbStore((s) => queryFor(s, s.activeConnId).error)
  const elapsedMs = useDbStore((s) => queryFor(s, s.activeConnId).elapsedMs)
  const sortBy = useDbStore((s) => queryFor(s, s.activeConnId).sortBy)
  const runQuery = useDbStore((s) => s.runQuery)
  const sortResult = useDbStore((s) => s.sortResult)

  // Mount the editor once. Seeds from the persisted sql; keystrokes flow back
  // into the store via setSql. Store actions are read through getState() so the
  // extensions never go stale (this effect runs a single time).
  useEffect(() => {
    if (!containerRef.current) return
    const st = useDbStore.getState()
    const state = EditorState.create({
      doc: queryFor(st, st.activeConnId).sql,
      extensions: [
        appTheme,
        dbEditorTheme,
        lineNumbers(),
        history(),
        sql(),
        syntaxHighlighting(appHighlightStyle),
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              void useDbStore.getState().runQuery()
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
  }, [])

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

  const canRun = sqlText.trim().length > 0 && !running

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-col">
        <div ref={containerRef} className="h-[160px] overflow-auto" />
        <div className="flex h-[40px] shrink-0 items-center gap-[8px] border-t border-b border-(--line-1) px-[10px]">
          <button
            type="button"
            onClick={() => void runQuery()}
            disabled={!canRun}
            className="flex h-[26px] items-center gap-[6px] rounded-[8px] bg-(--yz-accent) px-[10px] text-[12px] font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
          >
            <Play className="size-[13px]" aria-hidden="true" />
            {t("databasePanel.run")}
            <kbd className="rounded-[5px] bg-white/20 px-[5px] py-[1px] font-mono text-[10px]">⌘↵</kbd>
          </button>
          <div className="flex-1" />
          {elapsedMs != null && (
            <span className="font-mono text-[11px] text-(--ink-3)">{elapsedMs} ms</span>
          )}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <ResultView
          running={running}
          result={result}
          error={error}
          sortBy={sortBy}
          onSort={sortResult}
        />
      </div>
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
  error: string | null
  sortBy: DbSort | null
  onSort: (columnIndex: number) => void
}) {
  const { t } = useTranslation("panels")
  if (error) {
    return (
      <div className="m-[10px] rounded-[8px] border border-(--destructive)/40 bg-(--danger-soft) px-[10px] py-[8px] font-mono text-[12px] whitespace-pre-wrap text-(--destructive)">
        {error}
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
        {t("databasePanel.rowsAffected", { count: result.affectedRows })}
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
  const [dragPos, setDragPos] = useState<number | null>(null)

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse font-mono text-[12px]">
          <thead className="sticky top-0 bg-(--paper-1)">
            <tr>
              {order.map((origIdx, pos) => {
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
                {order.map((origIdx) => {
                  const v = row[origIdx]
                  return (
                    <td
                      key={origIdx}
                      className="border-r border-b border-(--line-1)/60 px-[10px] py-[4px] whitespace-nowrap text-(--ink-1) last:border-r-0"
                    >
                      {v === null ? <span className="text-(--ink-4) italic">NULL</span> : String(v)}
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
