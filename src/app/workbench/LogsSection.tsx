import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { homeDir, join } from "@tauri-apps/api/path"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { save } from "@tauri-apps/plugin-dialog"
import { openPath } from "@tauri-apps/plugin-opener"
import { Copy, Download, FolderOpen } from "lucide-react"

import { getLogLevel, logExport, logQuery, logSanitizeLines, logSources, setLogLevel, type LogQueryFilters } from "@/features/logs/logQuery"
import { groupRowsByRun, shortRunId, UNKNOWN_RUN } from "@/features/logs/runGroups"
import type { LogRecord, SanitizeSummary } from "@/lib/types"
import { cn } from "@/lib/utils"
import { SettingCard, SettingsTextInput } from "./settingsPrimitives"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"

// 必須與 Rust 的 logging::VALID_KINDS 一致，否則新 kind 的 record 永遠篩不出來。
const LOG_KIND_OPTIONS = ["debug", "user_action", "audit", "app_lifecycle"]
const LOG_LEVEL_OPTIONS = ["debug", "info", "warn", "error"]
const LOG_QUERY_LIMIT = 500
const LOG_RESULT_PAGE_SIZE = 50

function isValidIsoTimestamp(value: string): boolean {
  const trimmed = value.trim()
  if (!trimmed) return true

  const dateOnly = /^(\d{4})-(\d{2})-(\d{2})$/.exec(trimmed)
  if (dateOnly) {
    return isValidCalendarDate(dateOnly[1], dateOnly[2], dateOnly[3])
  }

  const datetimeLocal = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2})(?:\.\d{1,9})?)?$/.exec(trimmed)
  if (datetimeLocal) {
    return isValidCalendarDate(datetimeLocal[1], datetimeLocal[2], datetimeLocal[3])
      && isValidTime(datetimeLocal[4], datetimeLocal[5], datetimeLocal[6])
  }

  const rfc3339 = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(?:Z|[+-](\d{2}):(\d{2}))$/.exec(trimmed)
  return rfc3339 !== null
    && isValidCalendarDate(rfc3339[1], rfc3339[2], rfc3339[3])
    && isValidTime(rfc3339[4], rfc3339[5], rfc3339[6])
    && (rfc3339[7] === undefined || Number(rfc3339[7]) <= 23)
    && (rfc3339[8] === undefined || Number(rfc3339[8]) <= 59)
}

function isValidCalendarDate(yearText: string, monthText: string, dayText: string): boolean {
  const year = Number(yearText)
  const month = Number(monthText)
  const day = Number(dayText)
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0)
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
  return month >= 1 && month <= 12 && day >= 1 && day <= daysInMonth[month - 1]
}

function isValidTime(hourText: string, minuteText: string, secondText = "0"): boolean {
  return Number(hourText) <= 23 && Number(minuteText) <= 59 && Number(secondText) <= 59
}

function toggleFilterValue(values: string[], value: string): string[] {
  return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]
}

function metadataJson(metadata: unknown): string {
  try {
    return JSON.stringify(metadata, null, 2) ?? "null"
  } catch {
    return String(metadata)
  }
}

// log_sanitize_lines returns one redacted JSON line per row (including the
// fail-closed placeholder for lines it could not parse); keep the copied payload
// an array of objects so its shape matches the raw-mode copy.
function parseRedactedRow(line: string): unknown {
  try {
    return JSON.parse(line)
  } catch {
    return line
  }
}

function buildLogFilters({
  selectedKinds,
  selectedLevels,
  source,
  text,
  since,
  until,
}: {
  selectedKinds: string[]
  selectedLevels: string[]
  source: string
  text: string
  since: string
  until: string
}): LogQueryFilters {
  const filters: LogQueryFilters = { limit: LOG_QUERY_LIMIT }
  const query = text.trim()
  const from = since.trim()
  const to = until.trim()
  if (from) filters.since = from
  if (to) filters.until = to
  if (selectedLevels.length > 0) filters.levels = selectedLevels
  if (selectedKinds.length > 0) filters.kinds = selectedKinds
  if (source) filters.sources = [source]
  if (query) filters.text = query
  return filters
}

export function LogsSection({
  initialSource,
  openNonce,
}: {
  initialSource?: string
  openNonce?: number
}) {
  const { t } = useTranslation("workbench")
  const [rows, setRows] = useState<LogRecord[]>([])
  const [sources, setSources] = useState<string[]>([])
  const [selectedKinds, setSelectedKinds] = useState<string[]>([])
  const [selectedLevels, setSelectedLevels] = useState<string[]>([])
  const [source, setSource] = useState(initialSource ?? "")
  const [text, setText] = useState("")
  const [since, setSince] = useState("")
  const [until, setUntil] = useState("")
  const [sanitize, setSanitize] = useState(true)
  const [sanitizeSummary, setSanitizeSummary] = useState<SanitizeSummary | null>(null)
  const [runFilter, setRunFilter] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [verbose, setVerbose] = useState(false)
  const [resultPage, setResultPage] = useState(0)
  const sinceError = isValidIsoTimestamp(since)
    ? null
    : t("settings.logs.timeInvalid", { field: "since" })
  const untilError = isValidIsoTimestamp(until)
    ? null
    : t("settings.logs.timeInvalid", { field: "until" })

  useEffect(() => {
    let alive = true
    void logSources()
      .then((items) => {
        if (alive) setSources(items)
      })
      .catch((e) => {
        if (alive) setError(t("settings.logs.sourcesFailed", { error: String(e) }))
      })
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    let alive = true
    void getLogLevel()
      .then((level) => {
        if (alive) setVerbose(level === "debug")
      })
      .catch(() => {
        /* 讀不到就維持預設關閉 */
      })
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    setSource(initialSource ?? "")
  }, [initialSource, openNonce])

  useEffect(() => {
    if (!isValidIsoTimestamp(since) || !isValidIsoTimestamp(until)) {
      setLoading(false)
      return
    }
    let alive = true
    const timer = setTimeout(() => {
      const filters = buildLogFilters({ selectedKinds, selectedLevels, source, text, since, until })
      setLoading(true)
      setError(null)
      void logQuery(filters)
        .then((records) => {
          if (!alive) return
          setRows(records)
          setResultPage(0)
          setExpanded({})
          // run 分組是從**這一批結果**導出的，因此每次重新查詢都必須清掉 run
          // 篩選：留著上一批的 run id 會讓畫面變成空清單，而使用者看不出原因。
          setRunFilter(null)
        })
        .catch((e) => {
          if (alive) setError(t("settings.logs.queryFailed", { error: String(e) }))
        })
        .finally(() => {
          if (alive) setLoading(false)
        })
    }, 300)
    return () => {
      alive = false
      clearTimeout(timer)
    }
  }, [selectedKinds, selectedLevels, source, text, since, until])

  // run 分組與 run 篩選都作用在**已載入的這一批 rows** 上（client-side）：run_id
  // 不是 log_query 的伺服端 filter，而 Logs pane 一次最多載入 LOG_QUERY_LIMIT 筆。
  const runGroups = groupRowsByRun(rows)
  const visibleRows =
    runFilter === null ? rows : rows.filter((row) => (row.run_id ?? UNKNOWN_RUN) === runFilter)
  const resultPageCount = Math.max(1, Math.ceil(visibleRows.length / LOG_RESULT_PAGE_SIZE))
  const activeResultPage = Math.min(resultPage, resultPageCount - 1)
  const pageRows = visibleRows.slice(
    activeResultPage * LOG_RESULT_PAGE_SIZE,
    (activeResultPage + 1) * LOG_RESULT_PAGE_SIZE
  )

  // Copy 與 Export 走同一條 Rust redaction（log_sanitize_lines / log_export），
  // sanitize checkbox 對兩者的語意一致。
  async function copyRows() {
    setError(null)
    setNotice(null)
    try {
      // 複製目前結果頁看得到的 rows；run 篩選與分頁都必須反映在 clipboard。
      const payload = sanitize
        ? (await logSanitizeLines(pageRows.map((row) => JSON.stringify(row)))).map(
            parseRedactedRow
          )
        : pageRows
      await writeText(JSON.stringify(payload, null, 2))
      setNotice(
        sanitize
          ? t("settings.logs.copiedSanitized", { count: pageRows.length })
          : t("settings.logs.copiedRaw", { count: pageRows.length })
      )
    } catch (e) {
      setError(t("settings.logs.copyFailed", { error: String(e) }))
    }
  }

  async function exportBundle() {
    setError(null)
    setNotice(null)
    setSanitizeSummary(null)
    try {
      const dest = await save({
        title: t("settings.logs.exportDialogTitle"),
        defaultPath: "Yuzora-logs.zip",
        filters: [{ name: "ZIP", extensions: ["zip"] }],
        canCreateDirectories: true,
      })
      if (!dest) return
      const exported = await logExport(dest, sanitize)
      setSanitizeSummary(exported.summary)
      setNotice(t("settings.logs.exported", { path: exported.path }))
    } catch (e) {
      setError(t("settings.logs.exportFailed", { error: String(e) }))
    }
  }

  async function openLogsFolder() {
    setError(null)
    setNotice(null)
    try {
      const dir = await join(await homeDir(), ".yuzora", "logs")
      await openPath(dir)
      setNotice(t("settings.logs.openedFolder", { path: dir }))
    } catch (e) {
      setError(t("settings.logs.openFolderFailed", { error: String(e) }))
    }
  }

  async function toggleVerbose(next: boolean) {
    setVerbose(next)
    setError(null)
    try {
      await setLogLevel(next ? "debug" : "info")
      setNotice(next ? t("settings.logs.verboseEnabled") : t("settings.logs.verboseDisabled"))
    } catch (e) {
      setVerbose(!next) // 失敗回滾 UI
      setError(t("settings.logs.logLevelFailed", { error: String(e) }))
    }
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label={t("settings.logs.filters")} sub={t("settings.logs.filtersSub")}>
        <div className="flex flex-col gap-[12px]">
          <div className="grid grid-cols-2 gap-[12px]">
            <div role="group" aria-label={t("settings.logs.kindFilter")} className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">kind</span>
              <div className="flex flex-wrap gap-[6px]">
                {LOG_KIND_OPTIONS.map((kind) => {
                  const active = selectedKinds.includes(kind)
                  return (
                    <button
                      key={kind}
                      type="button"
                      aria-pressed={active}
                      onClick={() => setSelectedKinds((prev) => toggleFilterValue(prev, kind))}
                      className={cn(
                        "h-[26px] rounded-[8px] border px-[9px] font-mono text-[11px] transition-colors",
                        active
                          ? "border-(--yz-accent) bg-(--yz-sunk) font-semibold text-(--ink-1)"
                          : "border-(--line-1) text-(--ink-2) hover:bg-(--yz-hover)"
                      )}
                    >
                      {kind}
                    </button>
                  )
                })}
              </div>
            </div>

            <div role="group" aria-label={t("settings.logs.levelFilter")} className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">level</span>
              <div className="flex flex-wrap gap-[6px]">
                {LOG_LEVEL_OPTIONS.map((level) => {
                  const active = selectedLevels.includes(level)
                  return (
                    <button
                      key={level}
                      type="button"
                      aria-pressed={active}
                      onClick={() => setSelectedLevels((prev) => toggleFilterValue(prev, level))}
                      className={cn(
                        "h-[26px] rounded-[8px] border px-[9px] font-mono text-[11px] transition-colors",
                        active
                          ? "border-(--yz-accent) bg-(--yz-sunk) font-semibold text-(--ink-1)"
                          : "border-(--line-1) text-(--ink-2) hover:bg-(--yz-hover)"
                      )}
                    >
                      {level}
                    </button>
                  )
                })}
              </div>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-[12px]">
            <label className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">{t("settings.logs.source")}</span>
              <select
                aria-label={t("settings.logs.sourceFilter")}
                value={source}
                onChange={(event) => setSource(event.currentTarget.value)}
                className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
              >
                <option value="">{t("settings.logs.allSources")}</option>
                {sources.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            </label>

            <label className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">{t("settings.logs.textSearch")}</span>
              <input
                aria-label={t("settings.logs.textSearch")}
                type="search"
                value={text}
                placeholder={t("settings.logs.textSearchPlaceholder")}
                onChange={(event) => setText(event.currentTarget.value)}
                className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors placeholder:text-(--ink-4) focus:border-(--yz-accent)"
              />
            </label>
          </div>

          <div className="grid grid-cols-2 gap-[12px]">
            <SettingsTextInput
              label="since"
              value={since}
              placeholder="2026-01-02T00:00:00+08:00"
              onChange={setSince}
              error={sinceError}
              errorId="logs-since-error"
            />
            <SettingsTextInput
              label="until"
              value={until}
              placeholder="2026-01-03T00:00:00+08:00"
              onChange={setUntil}
              error={untilError}
              errorId="logs-until-error"
            />
          </div>
        </div>
      </SettingCard>

      <SettingCard label={t("settings.logs.actions")} sub={t("settings.logs.actionsSub")}>
        <div className="flex flex-wrap items-center gap-[8px]">
          <button
            type="button"
            onClick={() => void copyRows()}
            disabled={visibleRows.length === 0}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover) disabled:opacity-50"
          >
            <Copy className="size-[12px]" aria-hidden="true" />
            {t("settings.logs.copy")}
          </button>
          <button
            type="button"
            onClick={() => void exportBundle()}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
          >
            <Download className="size-[12px]" aria-hidden="true" />
            {t("settings.logs.exportBundle")}
          </button>
          <button
            type="button"
            onClick={() => void openLogsFolder()}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
          >
            <FolderOpen className="size-[12px]" aria-hidden="true" />
            {t("settings.logs.openLogsFolder")}
          </button>
          <label className="flex h-[28px] items-center gap-[7px] text-[11.5px] text-(--ink-2)">
            <input
              type="checkbox"
              checked={verbose}
              onChange={(event) => void toggleVerbose(event.currentTarget.checked)}
              className="size-[13px] accent-(--yz-accent)"
            />
            {t("settings.logs.verboseLogging")}
          </label>
          <label className="ml-auto flex h-[28px] items-center gap-[7px] text-[11.5px] text-(--ink-2)">
            <input
              type="checkbox"
              checked={sanitize}
              onChange={(event) => {
                setSanitize(event.currentTarget.checked)
                setSanitizeSummary(null)
              }}
              className="size-[13px] accent-(--yz-accent)"
            />
            {t("settings.logs.sanitize")}
          </label>
        </div>
        {sanitize ? (
          <div
            role="note"
            data-testid="logs-sanitize-preview"
            className="mt-[10px] flex flex-col gap-[3px] rounded-[8px] bg-(--yz-sunk) px-[9px] py-[7px] text-[11px] leading-[1.5] text-(--ink-3)"
          >
            <span>{t("settings.logsSanitizeKept")}</span>
            <span>{t("settings.logsSanitizeRemoved")}</span>
            {sanitizeSummary && (
              <span data-testid="logs-sanitize-counts">
                {t("settings.logsSanitizeCounts", {
                  paths: sanitizeSummary.paths,
                  hosts: sanitizeSummary.hosts,
                  usernames: sanitizeSummary.usernames,
                  fingerprints: sanitizeSummary.fingerprints,
                  secrets: sanitizeSummary.secrets,
                  unparseableLines: sanitizeSummary.unparseable_lines,
                })}
              </span>
            )}
          </div>
        ) : (
          <div
            role="note"
            data-testid="logs-raw-mode-warning"
            className="mt-[10px] rounded-[8px] bg-[#c2293f]/10 px-[9px] py-[7px] text-[11px] leading-[1.5] text-[#c2293f]"
          >
            {t("settings.logsRawModeWarning")}
          </div>
        )}
        {notice && (
          <div role="status" className="mt-[10px] text-[11px] text-(--ink-3)">
            {notice}
          </div>
        )}
        {error && (
          <div
            role="alert"
            className="mt-[10px] rounded-[8px] bg-[#c2293f]/10 px-[9px] py-[7px] text-[11px] leading-[1.5] text-[#c2293f]"
          >
            {error}
          </div>
        )}
      </SettingCard>

      <SettingCard
        label={t("settings.logs.results")}
        sub={loading
          ? t("settings.logs.loading")
          : t("settings.logs.resultSummary", {
              visible: visibleRows.length,
              total: rows.length,
              runs: runGroups.length,
            })}
      >
        {runGroups.length > 0 && (
          <div
            role="group"
            aria-label={t("settings.logs.runGroups")}
            data-testid="logs-run-groups"
            className="mb-[10px] flex flex-wrap gap-[6px]"
          >
            {runGroups.map((group) => {
              const active = runFilter === group.runId
              return (
                <button
                  key={group.runId}
                  type="button"
                  aria-pressed={active}
                  data-testid={`logs-run-group-${group.runId}`}
                  title={`${group.runId}\n${group.startedAt} → ${group.endedAt}`}
                  onClick={() => {
                    setRunFilter(active ? null : group.runId)
                    setResultPage(0)
                    setExpanded({})
                  }}
                  className={cn(
                    "h-[26px] rounded-[8px] border px-[9px] font-mono text-[11px] transition-colors",
                    active
                      ? "border-(--yz-accent) bg-(--yz-sunk) font-semibold text-(--ink-1)"
                      : "border-(--line-1) text-(--ink-2) hover:bg-(--yz-hover)"
                  )}
                >
                  run {shortRunId(group.runId)} · {group.count}
                </button>
              )
            })}
          </div>
        )}
        {resultPageCount > 1 ? (
          <div className="mb-[10px] flex items-center justify-end gap-[8px]">
            <Button
              type="button"
              variant="outline"
              size="xs"
              aria-label={t("settings.logs.previousPage")}
              disabled={activeResultPage === 0}
              onClick={() => setResultPage((page) => Math.max(0, page - 1))}
              className="h-[26px] rounded-[8px] border border-(--line-1) px-[9px] text-[11px] text-(--ink-2) disabled:opacity-50"
            >
              {t("settings.logs.previous")}
            </Button>
            <span className="text-[11px] text-(--ink-3)">
              {t("settings.logs.pageStatus", {
                page: activeResultPage + 1,
                pages: resultPageCount,
              })}
            </span>
            <Button
              type="button"
              variant="outline"
              size="xs"
              aria-label={t("settings.logs.nextPage")}
              disabled={activeResultPage >= resultPageCount - 1}
              onClick={() => setResultPage((page) => Math.min(resultPageCount - 1, page + 1))}
              className="h-[26px] rounded-[8px] border border-(--line-1) px-[9px] text-[11px] text-(--ink-2) disabled:opacity-50"
            >
              {t("settings.logs.next")}
            </Button>
          </div>
        ) : null}
        <ScrollArea
          orientation="horizontal"
          className="w-full"
        >
          <div role="list" aria-label={t("settings.logs.resultsList")} className="flex flex-col gap-[7px]">
          {visibleRows.length === 0 && !loading && (
            <div className="rounded-[8px] bg-(--yz-sunk) px-[10px] py-[12px] text-[11.5px] text-(--ink-3)">
              {t("settings.logs.noMatches")}
            </div>
          )}
          {pageRows.map((row, index) => {
            const resultIndex = activeResultPage * LOG_RESULT_PAGE_SIZE + index
            const key = `${row.timestamp}:${row.source}:${row.event}:${resultIndex}`
            const isExpanded = expanded[key] === true
            return (
              <div role="listitem" key={key} className="rounded-[10px] border border-(--line-1) bg-(--paper-0)">
                <button
                  type="button"
                  data-testid={`log-row-${row.event}`}
                  aria-label={t(
                    isExpanded ? "settings.logs.collapseMetadata" : "settings.logs.expandMetadata",
                    { event: row.event }
                  )}
                  onClick={() => setExpanded((prev) => ({ ...prev, [key]: !isExpanded }))}
                  className="grid w-full grid-cols-[minmax(88px,1.3fr)_62px_40px_56px_52px_minmax(64px,1fr)_minmax(77px,1.4fr)] items-center gap-[8px] px-[10px] py-[8px] text-left text-[11px] text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
                >
                  <span className="truncate font-mono text-(--ink-3)">{row.timestamp}</span>
                  <span
                    data-testid={`log-row-run-${row.event}`}
                    title={row.run_id ?? t("settings.logs.legacyRun")}
                    className="truncate font-mono text-(--ink-3)"
                  >
                    {row.run_id ? shortRunId(row.run_id) : UNKNOWN_RUN}
                  </span>
                  <span className="truncate font-mono font-semibold text-(--ink-1)">
                    {row.level}
                  </span>
                  <span className="truncate font-mono">{row.kind}</span>
                  <span className="truncate font-mono">{row.source}</span>
                  <span className="truncate font-mono text-(--ink-1)">{row.event}</span>
                  <span className="truncate">{row.message}</span>
                </button>
                {isExpanded && (
                  <ScrollArea orientation="horizontal" className="border-t border-(--line-1)" focusable>
                    <pre className="px-[10px] py-[9px] font-mono text-[10.5px] leading-[1.5] whitespace-pre-wrap text-(--ink-2)">
                      {metadataJson(row.metadata)}
                    </pre>
                  </ScrollArea>
                )}
              </div>
            )
          })}
          </div>
        </ScrollArea>
      </SettingCard>
    </div>
  )
}
