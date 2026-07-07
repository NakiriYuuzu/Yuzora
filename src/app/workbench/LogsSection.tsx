import { useEffect, useState } from "react"
import { homeDir, join } from "@tauri-apps/api/path"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { save } from "@tauri-apps/plugin-dialog"
import { openPath } from "@tauri-apps/plugin-opener"
import { Copy, Download, FolderOpen } from "lucide-react"

import { logExport, logQuery, logSources, type LogQueryFilters } from "@/features/logs/logQuery"
import type { LogRecord } from "@/lib/types"
import { cn } from "@/lib/utils"
import { SettingCard, SettingsTextInput } from "./settingsPrimitives"

const LOG_KIND_OPTIONS = ["debug", "user_action", "audit"]
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
  const [rows, setRows] = useState<LogRecord[]>([])
  const [sources, setSources] = useState<string[]>([])
  const [selectedKinds, setSelectedKinds] = useState<string[]>([])
  const [selectedLevels, setSelectedLevels] = useState<string[]>([])
  const [source, setSource] = useState(initialSource ?? "")
  const [text, setText] = useState("")
  const [since, setSince] = useState("")
  const [until, setUntil] = useState("")
  const [sanitize, setSanitize] = useState(true)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

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
          if (alive) setRows(records)
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

  async function copyRows() {
    setError(null)
    setNotice(null)
    try {
      await writeText(JSON.stringify(rows, null, 2))
      setNotice(`已複製 ${rows.length} rows`)
    } catch (e) {
      setError(`Copy 失敗：${String(e)}`)
    }
  }

  async function exportBundle() {
    setError(null)
    setNotice(null)
    try {
      const dest = await save({
        title: "Export logs bundle",
        defaultPath: "yuzora-logs.zip",
        filters: [{ name: "ZIP", extensions: ["zip"] }],
        canCreateDirectories: true,
      })
      if (!dest) return
      const exported = await logExport(dest, sanitize)
      setNotice(`已匯出：${exported}`)
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
            disabled={rows.length === 0}
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
          <label className="ml-auto flex h-[28px] items-center gap-[7px] text-[11.5px] text-(--ink-2)">
            <input
              type="checkbox"
              checked={sanitize}
              onChange={(event) => setSanitize(event.currentTarget.checked)}
              className="size-[13px] accent-(--yz-accent)"
            />
            sanitize
          </label>
        </div>
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

      <SettingCard label="結果" sub={loading ? "載入中..." : `${rows.length} rows`}>
        <div className="flex flex-col gap-[7px] overflow-x-auto">
          {rows.length === 0 && !loading && (
            <div className="rounded-[8px] bg-(--yz-sunk) px-[10px] py-[12px] text-[11.5px] text-(--ink-3)">
              沒有符合 filters 的 logs。
            </div>
          )}
          {rows.map((row, index) => {
            const key = `${row.timestamp}:${row.source}:${row.event}:${index}`
            const isExpanded = expanded[key] === true
            return (
              <div key={key} className="rounded-[10px] border border-(--line-1) bg-(--paper-0)">
                <button
                  type="button"
                  data-testid={`log-row-${row.event}`}
                  aria-label={`${isExpanded ? "收合" : "展開"} metadata ${row.event}`}
                  onClick={() => setExpanded((prev) => ({ ...prev, [key]: !isExpanded }))}
                  className="grid w-full grid-cols-[minmax(88px,1.3fr)_40px_56px_52px_minmax(64px,1fr)_minmax(77px,1.4fr)] items-center gap-[8px] px-[10px] py-[8px] text-left text-[11px] text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
                >
                  <span className="truncate font-mono text-(--ink-3)">{row.timestamp}</span>
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
