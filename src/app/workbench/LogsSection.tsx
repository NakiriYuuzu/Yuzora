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

// 必須與 Rust 的 logging::VALID_KINDS 一致，否則新 kind 的 record 永遠篩不出來。
const LOG_KIND_OPTIONS = ["debug", "user_action", "audit", "app_lifecycle"]
const LOG_LEVEL_OPTIONS = ["debug", "info", "warn", "error"]
const LOG_QUERY_LIMIT = 500

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

  useEffect(() => {
    let alive = true
    void logSources()
      .then((items) => {
        if (alive) setSources(items)
      })
      .catch((e) => {
        if (alive) setError(`log_sources 失敗：${String(e)}`)
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
    let alive = true
    const timer = setTimeout(() => {
      const filters = buildLogFilters({ selectedKinds, selectedLevels, source, text, since, until })
      setLoading(true)
      setError(null)
      void logQuery(filters)
        .then((records) => {
          if (!alive) return
          setRows(records)
          // run 分組是從**這一批結果**導出的，因此每次重新查詢都必須清掉 run
          // 篩選：留著上一批的 run id 會讓畫面變成空清單，而使用者看不出原因。
          setRunFilter(null)
        })
        .catch((e) => {
          if (alive) setError(`log_query 失敗：${String(e)}`)
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

  // Copy 與 Export 走同一條 Rust redaction（log_sanitize_lines / log_export），
  // sanitize checkbox 對兩者的語意一致。
  async function copyRows() {
    setError(null)
    setNotice(null)
    try {
      // 複製畫面上看得到的那些 rows——套了 run 篩選卻複製到全部，會與使用者
      // 眼前的內容不符。
      const payload = sanitize
        ? (await logSanitizeLines(visibleRows.map((row) => JSON.stringify(row)))).map(
            parseRedactedRow
          )
        : visibleRows
      await writeText(JSON.stringify(payload, null, 2))
      setNotice(
        sanitize
          ? `已複製 ${visibleRows.length} rows（已 sanitize）`
          : `已複製 ${visibleRows.length} rows（raw，未 sanitize）`
      )
    } catch (e) {
      setError(`Copy 失敗：${String(e)}`)
    }
  }

  async function exportBundle() {
    setError(null)
    setNotice(null)
    setSanitizeSummary(null)
    try {
      const dest = await save({
        title: "Export logs bundle",
        defaultPath: "Yuzora-logs.zip",
        filters: [{ name: "ZIP", extensions: ["zip"] }],
        canCreateDirectories: true,
      })
      if (!dest) return
      const exported = await logExport(dest, sanitize)
      setSanitizeSummary(exported.summary)
      setNotice(`已匯出：${exported.path}`)
    } catch (e) {
      setError(`Export bundle 失敗：${String(e)}`)
    }
  }

  async function openLogsFolder() {
    setError(null)
    setNotice(null)
    try {
      const dir = await join(await homeDir(), ".yuzora", "logs")
      await openPath(dir)
      setNotice(`已開啟 logs folder：${dir}`)
    } catch (e) {
      setError(`Open logs folder 失敗：${String(e)}`)
    }
  }

  async function toggleVerbose(next: boolean) {
    setVerbose(next)
    setError(null)
    try {
      await setLogLevel(next ? "debug" : "info")
      setNotice(next ? "已開啟 verbose logging（debug 會落盤）" : "已關閉 verbose logging")
    } catch (e) {
      setVerbose(!next) // 失敗回滾 UI
      setError(`設定 log level 失敗：${String(e)}`)
    }
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label="篩選" sub="log_query filters">
        <div className="flex flex-col gap-[12px]">
          <div className="grid grid-cols-2 gap-[12px]">
            <div role="group" aria-label="kind 篩選" className="flex flex-col gap-[6px]">
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

            <div role="group" aria-label="level 篩選" className="flex flex-col gap-[6px]">
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
              <span className="text-[11.5px] font-medium text-(--ink-2)">source</span>
              <select
                aria-label="source 篩選"
                value={source}
                onChange={(event) => setSource(event.currentTarget.value)}
                className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
              >
                <option value="">全部 sources</option>
                {sources.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            </label>

            <label className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">文字搜尋</span>
              <input
                aria-label="文字搜尋"
                type="search"
                value={text}
                placeholder="event 或 message"
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
            />
            <SettingsTextInput
              label="until"
              value={until}
              placeholder="2026-01-03T00:00:00+08:00"
              onChange={setUntil}
            />
          </div>
        </div>
      </SettingCard>

      <SettingCard label="動作" sub="Copy / Export bundle / Open logs folder">
        <div className="flex flex-wrap items-center gap-[8px]">
          <button
            type="button"
            onClick={() => void copyRows()}
            disabled={visibleRows.length === 0}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover) disabled:opacity-50"
          >
            <Copy className="size-[12px]" aria-hidden="true" />
            Copy
          </button>
          <button
            type="button"
            onClick={() => void exportBundle()}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
          >
            <Download className="size-[12px]" aria-hidden="true" />
            Export bundle
          </button>
          <button
            type="button"
            onClick={() => void openLogsFolder()}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
          >
            <FolderOpen className="size-[12px]" aria-hidden="true" />
            Open logs folder
          </button>
          <label className="flex h-[28px] items-center gap-[7px] text-[11.5px] text-(--ink-2)">
            <input
              type="checkbox"
              checked={verbose}
              onChange={(event) => void toggleVerbose(event.currentTarget.checked)}
              className="size-[13px] accent-(--yz-accent)"
            />
            Verbose logging (debug)
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
            sanitize
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
        label="結果"
        sub={loading ? "載入中..." : `${visibleRows.length} / ${rows.length} rows · ${runGroups.length} runs`}
      >
        {runGroups.length > 0 && (
          <div
            role="group"
            aria-label="app run 分組"
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
                  onClick={() => setRunFilter(active ? null : group.runId)}
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
        <div className="flex flex-col gap-[7px] overflow-x-auto">
          {visibleRows.length === 0 && !loading && (
            <div className="rounded-[8px] bg-(--yz-sunk) px-[10px] py-[12px] text-[11.5px] text-(--ink-3)">
              沒有符合 filters 的 logs。
            </div>
          )}
          {visibleRows.map((row, index) => {
            const key = `${row.timestamp}:${row.source}:${row.event}:${index}`
            const isExpanded = expanded[key] === true
            return (
              <div key={key} className="rounded-[10px] border border-(--line-1) bg-(--paper-0)">
                <button
                  type="button"
                  data-testid={`log-row-${row.event}`}
                  aria-label={`${isExpanded ? "收合" : "展開"} metadata ${row.event}`}
                  onClick={() => setExpanded((prev) => ({ ...prev, [key]: !isExpanded }))}
                  className="grid w-full grid-cols-[minmax(88px,1.3fr)_62px_40px_56px_52px_minmax(64px,1fr)_minmax(77px,1.4fr)] items-center gap-[8px] px-[10px] py-[8px] text-left text-[11px] text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
                >
                  <span className="truncate font-mono text-(--ink-3)">{row.timestamp}</span>
                  <span
                    data-testid={`log-row-run-${row.event}`}
                    title={row.run_id ?? "pre-#40 record（無 run id）"}
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
                  <pre className="overflow-x-auto border-t border-(--line-1) px-[10px] py-[9px] font-mono text-[10.5px] leading-[1.5] whitespace-pre-wrap text-(--ink-2)">
                    {metadataJson(row.metadata)}
                  </pre>
                )}
              </div>
            )
          })}
        </div>
      </SettingCard>
    </div>
  )
}
