import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { homeDir, join } from "@tauri-apps/api/path"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { save } from "@tauri-apps/plugin-dialog"
import { openPath } from "@tauri-apps/plugin-opener"
import {
  Bot,
  Check,
  Code,
  Copy,
  Download,
  Droplet,
  FileText,
  FolderOpen,
  GitBranch,
  Lock,
  MonitorPlay,
  RefreshCw,
  Server,
  Shield,
  TerminalSquare,
  Trash2,
  X,
} from "lucide-react"
import type { LucideIcon } from "lucide-react"

import {
  lspConfigClearStale,
  lspConfigGet,
  lspConfigSetServer,
  lspConfigStale,
  lspInstallServer,
  lspSetTrace,
  lspStatus,
  agentSetTrace,
} from "@/lib/ipc"
import { logExport, logQuery, logSources, type LogQueryFilters } from "@/features/logs/logQuery"
import type {
  LogRecord,
  LspConfig,
  LspInstallProgress,
  LspLanguage,
  LspServerInfo,
} from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { Switch } from "@/components/ui/switch"
import { FORMAT_ON_SAVE_STORAGE_KEY } from "@/editor/EditorPane"
import { strings } from "@/lib/i18n"
import { useGitStore, type RemoteCheckMode } from "@/state/gitStore"
import { useLspStore } from "@/state/lspStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

export type ThemePreference = "light" | "dark" | "auto"

interface SettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  theme: ThemePreference
  onThemeChange: (theme: ThemePreference) => void
  // Optional target applied whenever the dialog opens (or the target changes
  // while open): jump to a section and, for the LSP pane, focus a language card.
  // openSettings("lsp","python") drives these through AppShell (uiStore).
  initialSection?: string
  initialLanguage?: string
  // Bumped by every openSettings call. A dep of the sync effect so re-issuing the
  // SAME target (after the user manually navigated away) still re-applies it —
  // identical section/language primitives alone wouldn't re-fire the effect.
  openNonce?: number
}

type SectionId =
  | "appearance"
  | "editor"
  | "safety"
  | "git"
  | "lsp"
  | "agent"
  | "logs"
  | "terminal"
  | "preview"

// Design reference settings nav (§ settingsNav): three panes with icon rows.
const SECTIONS: { id: SectionId; label: string; sub: string; icon: LucideIcon }[] = [
  { id: "appearance", label: "Appearance", sub: "Theme and display language", icon: Droplet },
  { id: "editor", label: "Editor", sub: "Formatting and editor surface", icon: Code },
  { id: "lsp", label: "LSP", sub: "Language servers", icon: Server },
  { id: "agent", label: "Agent", sub: "Agent 啟動與 ACP trace", icon: Bot },
  { id: "logs", label: "Logs", sub: "Debug, user_action, audit events", icon: FileText },
  { id: "terminal", label: "Terminal", sub: "Shell and sessions", icon: TerminalSquare },
  { id: "preview", label: "Preview", sub: "Dev server and browser", icon: MonitorPlay },
  { id: "safety", label: "Safety", sub: "Guardrails for files and git", icon: Shield },
  { id: "git", label: "Git", sub: "Repository integration", icon: GitBranch },
]

export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const PREVIEW_SETTINGS_STORAGE_KEY = "yuzora:preview-settings"
export const AGENT_SETTINGS_STORAGE_KEY = "yuzora:agent-settings"
export const DEFAULT_AGENT_COMMAND = "bunx pi-acp"

type AgentPreset = "pi" | "custom"

export interface TerminalSettings {
  shellPath: string
  shellArgs: string
}

export interface PreviewSettings {
  command: string
  port: string
}

export interface AgentSettings {
  preset: AgentPreset
  command: string
  traceEnabled: boolean
}

const DEFAULT_TERMINAL_SETTINGS: TerminalSettings = {
  shellPath: "",
  shellArgs: "",
}

const DEFAULT_PREVIEW_SETTINGS: PreviewSettings = {
  command: "",
  port: "",
}

const DEFAULT_AGENT_SETTINGS: AgentSettings = {
  preset: "pi",
  command: DEFAULT_AGENT_COMMAND,
  traceEnabled: false,
}

function readJsonSetting<T extends object>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key)
    if (!raw) return fallback
    const parsed = JSON.parse(raw) as Partial<T>
    return { ...fallback, ...parsed }
  } catch {
    return fallback
  }
}

function writeJsonSetting<T extends object>(key: string, value: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* private mode / quota — keep the in-memory field value only */
  }
}

export function loadTerminalSettings(): TerminalSettings {
  return readJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, DEFAULT_TERMINAL_SETTINGS)
}

export function loadPreviewSettings(): PreviewSettings {
  return readJsonSetting(PREVIEW_SETTINGS_STORAGE_KEY, DEFAULT_PREVIEW_SETTINGS)
}

export function loadAgentSettings(): AgentSettings {
  const settings = readJsonSetting(AGENT_SETTINGS_STORAGE_KEY, DEFAULT_AGENT_SETTINGS)
  return {
    preset: settings.preset === "custom" ? "custom" : "pi",
    command: settings.command.trim() || DEFAULT_AGENT_COMMAND,
    traceEnabled: settings.traceEnabled === true,
  }
}

export function resolveAgentCommand(settings = loadAgentSettings()): string {
  if (settings.preset === "pi") return DEFAULT_AGENT_COMMAND
  return settings.command.trim() || DEFAULT_AGENT_COMMAND
}

// The four languages Yuzora ships LSP servers for (LspLanguage), with display
// labels for the settings cards.
const LSP_LANGUAGES: { id: LspLanguage; label: string }[] = [
  { id: "typescript", label: "TypeScript / JavaScript" },
  { id: "python", label: "Python" },
  { id: "rust", label: "Rust" },
  { id: "markdown", label: "Markdown" },
]

// Curated switchable server profiles per language. Static front-end mirror of the
// A9-curated set — source of truth: src-tauri/src/lsp_adapters.rs::all() (ids must
// match those adapter `id` strings). No runtime enumeration API exists, and this
// set is user-fixed (A9), so a static table is authoritative. First entry per
// language is the adapter default_id.
const LSP_PROFILES: Record<LspLanguage, { id: string; label: string }[]> = {
  typescript: [
    { id: "vtsls", label: "vtsls" },
    { id: "typescript-language-server", label: "typescript-language-server" },
  ],
  python: [
    { id: "pyright", label: "pyright" },
    { id: "pylsp", label: "pylsp" },
  ],
  rust: [{ id: "rust-analyzer", label: "rust-analyzer" }],
  markdown: [
    { id: "marksman", label: "marksman" },
    { id: "markdown-oxide", label: "markdown-oxide" },
  ],
}

// Remote-check modes for the Git pane — three-way aria-pressed segmented
// control (M1 Theme/Accent simplified radiogroup pattern). "probe" is default.
const REMOTE_CHECK_MODES: { id: RemoteCheckMode; label: string }[] = [
  { id: "off", label: "關閉" },
  { id: "probe", label: "唯讀檢查" },
  { id: "autofetch", label: "自動 fetch" },
]

// Reference §2.5 accent table (rgb/solid/ink). Only "lime" is selected here —
// accent switching itself is out of scope (brief §"不在範圍").
const ACCENT_SWATCHES: { id: string; solid: string }[] = [
  { id: "lime", solid: "#86b81f" },
  { id: "blue", solid: "#2f6bff" },
  { id: "violet", solid: "#7b5bff" },
  { id: "coral", solid: "#ff6b54" },
  { id: "amber", solid: "#e0a11f" },
]

/** Design reference settings card: --yz-panel surface, 13px radius. */
function SettingCard({
  label,
  sub,
  children,
}: {
  label: string
  sub?: string
  children: React.ReactNode
}) {
  return (
    <div className="rounded-[13px] border border-(--line-1) bg-(--yz-panel) p-[14px]">
      <div className={cn("text-[12.5px] font-medium text-(--ink-1)", !sub && "mb-[9px]")}>
        {label}
      </div>
      {sub && <div className="mt-[2px] mb-[9px] text-[11px] text-(--ink-3)">{sub}</div>}
      {children}
    </div>
  )
}

/** Design reference segmented control: sunken --paper-2 track, --yz-solid thumb. */
function Segmented({
  label,
  options,
  value,
  onChange,
}: {
  label: string
  options: { id: string; label: string }[]
  value: string
  onChange: (id: string) => void
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
    >
      {options.map((option) => {
        const active = option.id === value
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(option.id)}
            className={cn(
              "flex h-[28px] flex-1 items-center justify-center rounded-[8px] text-[11.5px] transition-all duration-[140ms] ease-(--ease-out)",
              active
                ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                : "font-medium text-(--ink-3) hover:text-(--ink-1)"
            )}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}

/** Design reference toggle row: label + sub on the left, switch on the right. */
function ToggleRow({
  label,
  sub,
  locked,
  checked,
  onCheckedChange,
}: {
  label: string
  sub: string
  locked?: boolean
  checked: boolean
  onCheckedChange: (checked: boolean) => void
}) {
  return (
    <div className="flex items-center gap-[12px] border-b border-(--line-1) px-[6px] py-[13px]">
      <div className="flex min-w-0 flex-1 flex-col gap-[2px]">
        <span className="flex items-center gap-[6px] text-[13px] font-medium text-(--ink-1)">
          {label}
          {locked && <Lock className="size-[11px] shrink-0 text-[#c2293f]" aria-hidden="true" />}
        </span>
        <span className="text-[11px] leading-[1.45] text-(--ink-3)">{sub}</span>
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onCheckedChange}
        aria-label={label}
        className="yz-switch"
      />
    </div>
  )
}

/**
 * Git pane — detection state card + remote-check card. Reads the live git
 * environment / remote-check config from useGitStore (T11) and re-detects
 * against the current workspace path. Visual language extends SettingCard;
 * no upstream design for this pane.
 */
function GitSection() {
  const environment = useGitStore((s) => s.environment)
  const remoteCheck = useGitStore((s) => s.remoteCheck)
  const setRemoteCheck = useGitStore((s) => s.setRemoteCheck)
  const detect = useGitStore((s) => s.detect)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  // Free-typing draft for the interval field: onChange no longer rejects
  // intermediate sub-minimum values (e.g. "4" on the way to "45"); clamping and
  // commit happen on blur (T19).
  const [intervalText, setIntervalText] = useState(String(remoteCheck.intervalSec))

  const redetect = () => {
    if (workspacePath) void detect(workspacePath)
  }

  function commitInterval() {
    const next = Number(intervalText)
    const clamped = Number.isFinite(next) && next >= 30 ? Math.floor(next) : 30
    setIntervalText(String(clamped))
    if (clamped !== remoteCheck.intervalSec) {
      setRemoteCheck({ ...remoteCheck, intervalSec: clamped })
    }
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label="偵測狀態" sub="Git executable and repository root">
        {(!environment || environment.status === "missing") && (
          <div className="flex flex-col gap-[10px]">
            <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
              {environment?.status === "missing" ? environment.reason : "尚未偵測 Git"}
            </span>
            <div>
              <button
                type="button"
                onClick={redetect}
                className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
              >
                <RefreshCw className="size-[12px]" aria-hidden="true" />
                重新偵測
              </button>
            </div>
          </div>
        )}

        {environment?.status === "notARepo" && (
          <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
            目前的工作區不是 Git repository。
          </span>
        )}

        {environment?.status === "ready" && (
          <div className="flex items-center gap-[8px]">
            <span className="size-[8px] shrink-0 rounded-full bg-(--yz-accent)" aria-hidden="true" />
            <span className="truncate font-mono text-[11.5px] text-(--ink-1)">
              git {environment.version} · {environment.root}
            </span>
          </div>
        )}
      </SettingCard>

      <SettingCard label="遠端檢查" sub="How Yuzora looks for upstream changes">
        <div
          role="group"
          aria-label="遠端檢查"
          className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
        >
          {REMOTE_CHECK_MODES.map((option) => {
            const active = option.id === remoteCheck.mode
            return (
              <button
                key={option.id}
                type="button"
                aria-pressed={active}
                onClick={() => setRemoteCheck({ ...remoteCheck, mode: option.id })}
                className={cn(
                  "flex h-[28px] flex-1 items-center justify-center rounded-[8px] text-[11.5px] transition-all duration-[140ms] ease-(--ease-out)",
                  active
                    ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                    : "font-medium text-(--ink-3) hover:text-(--ink-1)"
                )}
              >
                {option.label}
              </button>
            )
          })}
        </div>

        <label className="mt-[12px] flex items-center justify-between gap-[10px]">
          <span className="text-[12px] text-(--ink-2)">檢查間隔</span>
          <span className="flex items-center gap-[6px]">
            <input
              type="number"
              min={30}
              value={intervalText}
              onChange={(e) => setIntervalText(e.target.value)}
              onBlur={commitInterval}
              className="h-[28px] w-[76px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-right font-mono text-[11.5px] text-(--ink-1) tabular-nums outline-none focus:border-(--yz-accent)"
            />
            <span className="text-[11px] text-(--ink-3)">秒</span>
          </span>
        </label>
      </SettingCard>
    </div>
  )
}

function SettingsTextInput({
  label,
  value,
  onChange,
  placeholder,
  disabled = false,
  type = "text",
}: {
  label: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  disabled?: boolean
  type?: "text" | "number"
}) {
  return (
    <label className="flex flex-col gap-[6px]">
      <span className="text-[11.5px] font-medium text-(--ink-2)">{label}</span>
      <input
        aria-label={label}
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(event) => onChange(event.currentTarget.value)}
        className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] font-mono text-[11.5px] text-(--ink-1) outline-none transition-colors placeholder:text-(--ink-4) focus:border-(--yz-accent) disabled:cursor-not-allowed disabled:opacity-60"
      />
    </label>
  )
}

function TerminalSection() {
  const [settings, setSettings] = useState(loadTerminalSettings)

  const update = (patch: Partial<TerminalSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, next)
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard
        label={strings.terminal.shellLabel}
        sub={strings.terminal.shellDescription}
      >
        <div className="flex flex-col gap-[12px]">
          <SettingsTextInput
            label="Shell path override"
            value={settings.shellPath}
            placeholder="/opt/homebrew/bin/fish"
            onChange={(shellPath) => update({ shellPath })}
          />
          <SettingsTextInput
            label="Default shell args"
            value={settings.shellArgs}
            placeholder="-l"
            onChange={(shellArgs) => update({ shellArgs })}
          />
        </div>
      </SettingCard>
    </div>
  )
}

function PreviewSection() {
  const [settings, setSettings] = useState(loadPreviewSettings)

  const update = (patch: Partial<PreviewSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeJsonSetting(PREVIEW_SETTINGS_STORAGE_KEY, next)
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard
        label={strings.preview.devServerLabel}
        sub={strings.preview.devServerDescription}
      >
        <div className="flex flex-col gap-[12px]">
          <SettingsTextInput
            label="Dev server command override"
            value={settings.command}
            placeholder="bun run dev"
            onChange={(command) => update({ command })}
          />
          <SettingsTextInput
            label="Port override"
            type="number"
            value={settings.port}
            placeholder="5173"
            onChange={(port) => update({ port })}
          />
        </div>
      </SettingCard>
    </div>
  )
}

function AgentSection() {
  const [settings, setSettings] = useState(loadAgentSettings)
  const traceGenRef = useRef(0)

  const persist = (next: AgentSettings) => {
    setSettings(next)
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, next)
  }

  const update = (patch: Partial<AgentSettings>) => {
    persist({ ...settings, ...patch })
  }

  async function toggleTrace(next: boolean) {
    const prev = settings
    const gen = ++traceGenRef.current
    persist({ ...settings, traceEnabled: next })
    try {
      await agentSetTrace(next)
    } catch {
      if (gen === traceGenRef.current) persist(prev)
    }
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label="Agent 啟動" sub="ACP agent process command">
        <div className="flex flex-col gap-[12px]">
          <label className="flex flex-col gap-[6px]">
            <span className="text-[11.5px] font-medium text-(--ink-2)">Agent preset</span>
            <select
              aria-label="Agent preset"
              value={settings.preset}
              onChange={(event) => update({ preset: event.currentTarget.value as AgentPreset })}
              className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
            >
              <option value="pi">pi · bunx pi-acp</option>
              <option value="custom">自訂</option>
            </select>
          </label>
          <SettingsTextInput
            label="自訂 command"
            value={settings.preset === "pi" ? DEFAULT_AGENT_COMMAND : settings.command}
            placeholder={DEFAULT_AGENT_COMMAND}
            disabled={settings.preset !== "custom"}
            onChange={(command) => update({ command })}
          />
        </div>
      </SettingCard>

      <div className="flex flex-col">
        <ToggleRow
          label="ACP trace"
          sub="將 ACP JSON-RPC 原始行寫入 debug log（最多 500 字）"
          checked={settings.traceEnabled}
          onCheckedChange={toggleTrace}
        />
      </div>
    </div>
  )
}

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

function LogsSection({
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
    return () => {
      alive = false
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
        <div className="flex flex-col gap-[7px]">
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
                  className="grid w-full grid-cols-[minmax(116px,1.3fr)_52px_82px_70px_minmax(92px,1fr)_minmax(130px,1.4fr)] items-center gap-[8px] px-[10px] py-[8px] text-left text-[11px] text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
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
                  <pre className="border-t border-(--line-1) px-[10px] py-[9px] font-mono text-[10.5px] leading-[1.5] whitespace-pre-wrap text-(--ink-2)">
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

// localStorage read for the format-on-save switch. Mirrors EditorPane's own reader
// (same imported key) so the Settings toggle and the save path stay in lockstep;
// default OFF when unset (A7).
function loadFormatOnSave(): boolean {
  try {
    return localStorage.getItem(FORMAT_ON_SAVE_STORAGE_KEY) === "true"
  } catch {
    return false
  }
}

const BADGE_TONE: Record<"ok" | "warn" | "err" | "idle", string> = {
  ok: "bg-(--yz-sunk) text-(--yz-accent)",
  warn: "bg-(--yz-sunk) text-(--ink-2)",
  err: "bg-[#c2293f]/12 text-[#c2293f]",
  idle: "bg-(--yz-sunk) text-(--ink-3)",
}

// Process status → card badge. "ready" is a client-side notion (the process stays
// "starting" once spawned), so an initialized server reads as ready — mirroring
// lspStore.deriveDisplayState minus the per-file grade this pane doesn't have.
function statusBadge(
  info: LspServerInfo | undefined,
  initialized: boolean,
): { text: string; tone: "ok" | "warn" | "err" | "idle"; detail?: string } {
  if (!info) return { text: "尚未啟動", tone: "idle" }
  const s = info.status
  if (s.status === "missing") return { text: "未安裝", tone: "err", detail: s.installHint }
  if (s.status === "crashed") return { text: "已崩潰", tone: "err", detail: s.reason }
  if (s.status === "stopped") return { text: "已停止", tone: "idle" }
  return initialized ? { text: "就緒", tone: "ok" } : { text: "啟動中", tone: "warn" }
}

/**
 * One language card: active server + resolved binary + install state + startup
 * log / last error, with guided-install and re-detect actions. Data comes only
 * from lspStore (live info) + the parent's config read; there is no frontend
 * catalog wrapper, so the active server is read-only (no switch control) and a
 * bare card ("尚未啟動") is normal until a file mounts or an install resolves.
 */
function LspLanguageCard({
  language,
  label,
  highlighted,
  activeServer,
  info,
  initialized,
  progress,
  error,
  profiles,
  activeProfile,
  onSetProfile,
  onInstall,
  onRedetect,
}: {
  language: LspLanguage
  label: string
  highlighted: boolean
  activeServer: string | null
  info: LspServerInfo | undefined
  initialized: boolean
  progress: LspInstallProgress | null
  error: string | null
  profiles: { id: string; label: string }[]
  activeProfile: string | undefined
  onSetProfile: (id: string) => void
  onInstall: () => void
  onRedetect: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const openSettings = useUiStore((s) => s.openSettings)
  const badge = statusBadge(info, initialized)
  const installing = progress != null
  const ready = badge.tone === "ok"
  const failed = info?.status.status === "crashed"

  // Scroll the targeted card into view when the pane opens on it (openSettings
  // language target). scrollIntoView is a no-op stub under jsdom.
  useEffect(() => {
    if (highlighted) ref.current?.scrollIntoView({ block: "nearest" })
  }, [highlighted])

  return (
    <div
      ref={ref}
      data-testid={`lsp-card-${language}`}
      data-highlighted={highlighted ? "true" : undefined}
      className={cn(
        "rounded-[13px] border bg-(--yz-panel) p-[14px]",
        highlighted ? "border-(--yz-accent)" : "border-(--line-1)"
      )}
    >
      <div className="flex items-center justify-between gap-[10px]">
        <span className="text-[13px] font-medium text-(--ink-1)">{label}</span>
        <span
          className={cn(
            "rounded-[6px] px-[7px] py-[3px] text-[10.5px] font-medium",
            BADGE_TONE[badge.tone]
          )}
        >
          {badge.text}
        </span>
      </div>

      <dl className="mt-[10px] flex flex-col gap-[6px]">
        <div className="flex items-center justify-between gap-[10px]">
          <dt className="shrink-0 text-[11px] text-(--ink-3)">使用中伺服器</dt>
          <dd className="truncate font-mono text-[11.5px] text-(--ink-1)">
            {activeServer ?? "預設"}
          </dd>
        </div>
        {info?.path && (
          <div className="flex items-center justify-between gap-[10px]">
            <dt className="shrink-0 text-[11px] text-(--ink-3)">執行檔路徑</dt>
            <dd className="truncate font-mono text-[11px] text-(--ink-2)">{info.path}</dd>
          </div>
        )}
      </dl>

      <div className="mt-[10px]">
        <div className="mb-[6px] text-[11px] text-(--ink-3)">伺服器</div>
        <div role="radiogroup" aria-label={`${label} 伺服器`} className="flex flex-wrap gap-[6px]">
          {profiles.map((p) => {
            const single = profiles.length === 1
            const selected = p.id === activeProfile
            return (
              <button
                key={p.id}
                type="button"
                role="radio"
                aria-checked={selected}
                disabled={single}
                onClick={() => onSetProfile(p.id)}
                className={cn(
                  "flex h-[26px] items-center rounded-[8px] border px-[10px] font-mono text-[11px] transition-colors",
                  selected
                    ? "border-(--yz-accent) bg-(--yz-sunk) font-medium text-(--ink-1)"
                    : "border-(--line-1) text-(--ink-2) hover:bg-(--yz-hover)",
                  single && "cursor-default opacity-70"
                )}
              >
                {p.label}
              </button>
            )
          })}
        </div>
      </div>

      {badge.detail && (
        <div className="mt-[9px] rounded-[8px] bg-(--yz-sunk) px-[9px] py-[7px] font-mono text-[11px] leading-[1.5] text-(--ink-2)">
          {badge.detail}
        </div>
      )}

      {info?.lastStartupLog && (
        <details className="mt-[9px]">
          <summary className="cursor-pointer text-[11px] text-(--ink-3)">最近啟動記錄</summary>
          <pre className="mt-[6px] overflow-x-auto rounded-[8px] bg-(--yz-sunk) px-[9px] py-[7px] font-mono text-[10.5px] leading-[1.5] whitespace-pre-wrap text-(--ink-2)">
            {info.lastStartupLog}
          </pre>
        </details>
      )}

      {info?.lastError && (
        <div className="mt-[9px] rounded-[8px] bg-[#c2293f]/10 px-[9px] py-[7px] font-mono text-[11px] leading-[1.5] text-[#c2293f]">
          {info.lastError}
        </div>
      )}

      {progress && (
        <div className="mt-[9px] text-[11px] text-(--ink-2)">
          安裝中 · {progress.phase}
          {progress.percent != null && ` · ${progress.percent}%`}
          {progress.message && <span className="text-(--ink-3)"> · {progress.message}</span>}
        </div>
      )}

      {error && !installing && (
        <div className="mt-[9px] rounded-[8px] bg-[#c2293f]/10 px-[9px] py-[7px] text-[11px] leading-[1.5] text-[#c2293f]">
          {error}
        </div>
      )}

      <div className="mt-[11px] flex items-center gap-[8px]">
        {!ready && (
          <button
            type="button"
            onClick={onInstall}
            disabled={installing}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover) disabled:opacity-60"
          >
            <Download className="size-[12px]" aria-hidden="true" />
            {installing ? "安裝中…" : "一鍵安裝"}
          </button>
        )}
        <button
          type="button"
          onClick={onRedetect}
          className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
        >
          <RefreshCw className="size-[12px]" aria-hidden="true" />
          重新偵測
        </button>
        {failed && (
          <button
            type="button"
            onClick={() => openSettings("logs", { source: "lsp" })}
            className="flex h-[28px] items-center gap-[6px] rounded-[8px] border border-(--line-1) px-[11px] text-[11.5px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
          >
            <FileText className="size-[12px]" aria-hidden="true" />
            檢視 logs
          </button>
        )}
      </div>
    </div>
  )
}

/**
 * LSP pane — behaviour switches (format-on-save, JSON-RPC trace), stale-override
 * cleanup, and one card per language. Reads live server info from useLspStore
 * (fed by LspBridge) plus the persisted config via lspConfigGet. The switchable
 * server catalog and the trace-file path have no frontend API, so the active
 * server is read-only and the trace note is descriptive (see T12 report gap).
 */
function LspSection({ targetLanguage }: { targetLanguage?: string }) {
  const servers = useLspStore((s) => s.servers)
  const initializedMap = useLspStore((s) => s.initialized)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  const [config, setConfig] = useState<LspConfig | null>(null)
  const [stale, setStale] = useState<string[]>([])
  // Trace lives in uiStore (in-memory, not persisted) so it survives this pane
  // unmounting/remounting on dialog close — a section-local useState would reset
  // to off and desync from the Rust side (A-F4). App restart naturally clears it.
  const trace = useUiStore((s) => s.traceEnabled)
  const setTrace = useUiStore((s) => s.setTraceEnabled)
  // Monotonic id for trace requests: a late reject only reverts when it is still
  // the latest request, so a superseded toggle isn't clobbered by a stale one
  // (R2A-F1 — three rapid toggles where the first rejects after the others land).
  const traceGenRef = useRef(0)
  const [formatOnSave, setFormatOnSave] = useState(loadFormatOnSave)
  const [installing, setInstalling] = useState<Record<string, LspInstallProgress | null>>({})
  const [installError, setInstallError] = useState<Record<string, string | null>>({})
  // Where a profile switch is written: a workspace override or the global default.
  // Defaults to workspace when one is open, else global (workspace choice disabled).
  const [scope, setScope] = useState<"workspace" | "global">(
    workspacePath ? "workspace" : "global"
  )

  // Pull the persisted config + stale-override list once the pane opens.
  const refreshConfig = () => {
    void Promise.all([lspConfigGet(), lspConfigStale()])
      .then(([cfg, st]) => {
        setConfig(cfg)
        setStale(st)
      })
      .catch(() => {
        /* config read failure leaves the last-known values; cards still render */
      })
  }
  useEffect(refreshConfig, [])

  // Install progress streams over lsp:install-progress (T14 emits). A terminal
  // phase (done/error) clears the entry — idempotent with handleInstall's finally
  // so a terminal event arriving AFTER the install promise settled can't wedge the
  // button disabled or hide the real error (A-F1); other phases reflect progress.
  useEffect(() => {
    const unlisten = listen<LspInstallProgress>("lsp:install-progress", (e) => {
      const terminal = e.payload.phase === "done" || e.payload.phase === "error"
      setInstalling((prev) => ({ ...prev, [e.payload.language]: terminal ? null : e.payload }))
    })
    return () => {
      unlisten.then((fn) => fn()).catch(() => {})
    }
  }, [])

  const activeServerId = (language: LspLanguage): string | null => {
    const live = servers[language]?.serverId
    if (live) return live
    const override = workspacePath ? config?.workspaces[workspacePath]?.[language] : undefined
    return override ?? config?.defaults[language] ?? null
  }

  // The profile highlighted per card, per the active scope: the workspace override
  // (may be unset → inherits global, no highlight), or the global default (falls
  // back to the adapter default = first curated profile when unset).
  const activeProfile = (language: LspLanguage): string | undefined => {
    if (scope === "workspace" && workspacePath) {
      return config?.workspaces[workspacePath]?.[language]
    }
    return config?.defaults[language] ?? LSP_PROFILES[language][0]?.id
  }

  async function setProfile(language: LspLanguage, id: string) {
    const ws = scope === "workspace" ? workspacePath : null
    try {
      const cfg = await lspConfigSetServer(ws, language, id)
      setConfig(cfg)
    } catch {
      /* ignore — leave the current selection; user can retry */
    }
  }

  async function handleInstall(language: LspLanguage) {
    // Snapshot the workspace this install was started for; only feed the result
    // into the store if the UI is still on that workspace at settle — otherwise a
    // late install for a workspace the user has left pollutes the new one's store
    // (same workspace-currency guard as lspManager R2-5 / LspBridge :31).
    const wsAtRequest = workspacePath ?? null
    setInstallError((p) => ({ ...p, [language]: null }))
    setInstalling((p) => ({
      ...p,
      [language]: { language, phase: "download", percent: null, message: null },
    }))
    try {
      // Pass the raw current workspace (canonicalization is Rust-side) so the
      // install resolves the workspace override, not just the global default
      // (W6A-F1); null when no workspace is open = global resolve.
      const info = await lspInstallServer(wsAtRequest, language)
      if (wsAtRequest === (useWorkspaceStore.getState().workspacePath ?? null)) {
        useLspStore.getState().setServerInfo(info)
      }
    } catch (e) {
      setInstallError((p) => ({ ...p, [language]: String(e) }))
    } finally {
      setInstalling((p) => ({ ...p, [language]: null }))
    }
  }

  function redetect() {
    refreshConfig()
    if (!workspacePath) return
    // Only surfaces already-started servers — there is no resolve-only probe API,
    // so an unstarted server stays "尚未啟動" until installed or a file mounts it.
    void lspStatus(workspacePath)
      .then((list) => {
        for (const inf of list) useLspStore.getState().setServerInfo(inf)
      })
      .catch(() => {})
  }

  async function clearStale(ws: string) {
    try {
      const cfg = await lspConfigClearStale(ws)
      setConfig(cfg)
    } catch {
      /* ignore — leave the entry so the user can retry */
    }
    setStale((prev) => prev.filter((w) => w !== ws))
  }

  // Rebind = re-apply the stale workspace's server mapping to the current
  // workspace, then clear the stale entry. Composed from existing wrappers
  // (no dedicated rebind IPC); falls back to clear-only when the mapping or a
  // current workspace is missing.
  async function rebindStale(ws: string) {
    const mapping = config?.workspaces[ws]
    if (workspacePath && mapping) {
      for (const [language, serverId] of Object.entries(mapping)) {
        try {
          await lspConfigSetServer(workspacePath, language, serverId)
        } catch {
          /* skip a mapping that no longer resolves; still clear below */
        }
      }
    }
    await clearStale(ws)
  }

  function toggleFormatOnSave(next: boolean) {
    try {
      localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, String(next))
    } catch {
      /* private mode / quota — keep the in-memory toggle only */
    }
    setFormatOnSave(next)
  }

  // Optimistically reflect the toggle, then revert if the Rust call fails so the
  // UI never claims a trace state the backend rejected — and the rejection is
  // caught, not left unhandled (A-F3).
  async function toggleTrace(next: boolean) {
    const prev = useUiStore.getState().traceEnabled
    const gen = ++traceGenRef.current
    setTrace(next)
    try {
      await lspSetTrace(next)
    } catch {
      // Only revert if this is still the latest request — a superseded toggle's
      // late reject must not clobber a newer value (R2A-F1).
      if (gen === traceGenRef.current) setTrace(prev)
    }
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <div className="flex flex-col">
        <ToggleRow
          label="儲存時自動格式化"
          sub="透過語言伺服器在存檔時套用格式化"
          checked={formatOnSave}
          onCheckedChange={toggleFormatOnSave}
        />
        <ToggleRow
          label="JSON-RPC 追蹤"
          sub="將 LSP 通訊寫入追蹤檔（重啟後自動關閉）"
          checked={trace}
          onCheckedChange={toggleTrace}
        />
      </div>

      {stale.length > 0 && (
        <div data-testid="lsp-stale">
          <SettingCard label="失效的工作區覆寫" sub="Overrides whose workspace no longer exists">
            <div className="flex flex-col gap-[8px]">
              {stale.map((ws) => (
                <div key={ws} className="flex items-center justify-between gap-[10px]">
                  <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-(--ink-2)">
                    {ws}
                  </span>
                  <div className="flex shrink-0 items-center gap-[6px]">
                    <button
                      type="button"
                      onClick={() => void rebindStale(ws)}
                      disabled={!workspacePath}
                      className="flex h-[26px] items-center rounded-[8px] border border-(--line-1) px-[9px] text-[11px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover) disabled:opacity-50"
                    >
                      重新綁定至目前工作區
                    </button>
                    <button
                      type="button"
                      onClick={() => void clearStale(ws)}
                      className="flex h-[26px] items-center gap-[5px] rounded-[8px] border border-(--line-1) px-[9px] text-[11px] font-medium text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
                    >
                      <Trash2 className="size-[11px]" aria-hidden="true" />
                      清除
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </SettingCard>
        </div>
      )}

      <SettingCard label="伺服器設定範圍" sub="Where a server choice is saved">
        <div
          role="radiogroup"
          aria-label="伺服器設定範圍"
          className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
        >
          {(
            [
              { id: "workspace", label: "此工作區" },
              { id: "global", label: "全域" },
            ] as const
          ).map((option) => {
            const disabled = option.id === "workspace" && !workspacePath
            const selected = option.id === scope
            return (
              <button
                key={option.id}
                type="button"
                role="radio"
                aria-checked={selected}
                disabled={disabled}
                onClick={() => setScope(option.id)}
                className={cn(
                  "flex h-[28px] flex-1 items-center justify-center rounded-[8px] text-[11.5px] transition-all duration-[140ms] ease-(--ease-out)",
                  selected
                    ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                    : "font-medium text-(--ink-3) hover:text-(--ink-1)",
                  disabled && "cursor-not-allowed opacity-45 hover:text-(--ink-3)"
                )}
              >
                {option.label}
              </button>
            )
          })}
        </div>
      </SettingCard>

      {LSP_LANGUAGES.map((lang) => (
        <LspLanguageCard
          key={lang.id}
          language={lang.id}
          label={lang.label}
          highlighted={lang.id === targetLanguage}
          activeServer={activeServerId(lang.id)}
          info={servers[lang.id]}
          initialized={initializedMap[lang.id] ?? false}
          progress={installing[lang.id] ?? null}
          error={installError[lang.id] ?? null}
          profiles={LSP_PROFILES[lang.id]}
          activeProfile={activeProfile(lang.id)}
          onSetProfile={(id) => void setProfile(lang.id, id)}
          onInstall={() => void handleInstall(lang.id)}
          onRedetect={redetect}
        />
      ))}
    </div>
  )
}

/**
 * Settings dialog — design reference settings modal: frost surface, header
 * with avatar, 198px left nav (Appearance / Editor / LSP / Safety / Git +
 * version footer) and a scrollable card pane. Theme + the LSP and Git panes are
 * live; the remaining controls hold local placeholder state until their features
 * land. The dialog remembers the last section across opens, but an external
 * target (initialSection/initialLanguage, from openSettings) overrides it.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  theme,
  onThemeChange,
  initialSection,
  initialLanguage,
  openNonce,
}: SettingsDialogProps) {
  const [section, setSection] = useState<SectionId>("appearance")
  const [targetLanguage, setTargetLanguage] = useState<string | undefined>(undefined)
  const [language, setLanguage] = useState("en")
  const [fontSize, setFontSize] = useState("13")
  const [minimap, setMinimap] = useState(false)
  const [reconcile, setReconcile] = useState(true)
  const [confirmGit, setConfirmGit] = useState(true)
  const settingsLogSource = useUiStore((s) => s.settingsLogSource)

  // Apply an external target on open, and again if the target changes while the
  // dialog stays mounted. `openNonce` (bumped per openSettings) is a dep so
  // re-issuing the SAME target after a manual nav still re-applies it. Manual nav
  // clicks don't touch the props, so they are never fought. A null section leaves
  // the remembered section (rail/palette path).
  useEffect(() => {
    if (!open) return
    const match = SECTIONS.find((s) => s.id === initialSection)
    if (match) setSection(match.id)
    setTargetLanguage(initialLanguage)
  }, [open, initialSection, initialLanguage, openNonce])

  // Manual section nav: switch section and drop any external language highlight
  // (A-F5 — otherwise re-entering the LSP pane keeps the last targeted card lit).
  const selectSection = (id: SectionId) => {
    setSection(id)
    setTargetLanguage(undefined)
  }

  const active = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0]

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="yz-diffin flex h-[556px] max-h-[86vh] w-[720px] max-w-[92vw] flex-col gap-0 overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--frost-light) p-0 shadow-(--shadow-xl) ring-0 [backdrop-filter:var(--blur-frost)] sm:max-w-[92vw]"
      >
        <div className="flex shrink-0 items-center gap-[11px] border-b border-(--line-1) px-[20px] py-[15px]">
          <span
            aria-hidden="true"
            className="flex size-[32px] shrink-0 items-center justify-center rounded-full bg-[image:var(--grad-dusk)] text-[13px] font-semibold text-white shadow-(--shadow-xs)"
          >
            Y
          </span>
          <div className="min-w-0 flex-1">
            <DialogTitle className="font-serif text-[18px] leading-[1.1] font-semibold text-(--ink-0)">
              Settings
            </DialogTitle>
            <DialogDescription className="mt-[1px] text-[11px] text-(--ink-3)">
              Yuuzu · yuzora workspace
            </DialogDescription>
          </div>
          <DialogClose
            aria-label="Close settings"
            className="flex size-[28px] shrink-0 items-center justify-center rounded-[8px] text-(--ink-3) transition-colors hover:bg-(--paper-2) hover:text-(--ink-1)"
          >
            <X className="size-[16px]" aria-hidden="true" />
          </DialogClose>
        </div>

        <div className="flex min-h-0 flex-1">
          <aside className="flex w-[198px] shrink-0 flex-col border-r border-(--line-1) bg-(--yz-panel) px-[11px] py-[14px]">
            {SECTIONS.map(({ id, label, icon: Icon }) => {
              const isActive = id === section
              return (
                <button
                  key={id}
                  type="button"
                  aria-pressed={isActive}
                  onClick={() => selectSection(id)}
                  className={cn(
                    "flex h-[37px] shrink-0 items-center gap-[9px] rounded-[9px] px-[11px] text-[13px] tracking-[-0.01em] transition-all duration-[130ms] ease-(--ease-out)",
                    isActive
                      ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                      : "font-medium text-(--ink-2) hover:bg-(--yz-hover)"
                  )}
                >
                  <span className="flex size-[22px] shrink-0 items-center justify-center">
                    <Icon className="size-[15px]" aria-hidden="true" />
                  </span>
                  <span className="min-w-0 flex-1 text-left">{label}</span>
                </button>
              )
            })}
            <div className="flex-1" />
            <div className="flex items-center gap-[7px] px-[10px] py-[8px]">
              <span
                aria-hidden="true"
                className="size-[6px] shrink-0 rounded-full bg-(--yz-accent)"
              />
              <span className="font-mono text-[10px] text-(--ink-3)">Yuzora v0.1.0</span>
            </div>
          </aside>

          <div className="yzs min-w-0 flex-1 overflow-auto px-[26px] pt-[22px] pb-[26px]">
            <h3 className="font-serif text-[17px] leading-[1.1] font-semibold text-(--ink-0)">
              {active.label}
            </h3>
            <div className="mt-[3px] mb-[18px] text-[11.5px] text-(--ink-3)">{active.sub}</div>

            {section === "appearance" && (
              <div className="flex flex-col gap-[14px]">
                <SettingCard label="Theme">
                  <Segmented
                    label="Theme"
                    options={[
                      { id: "light", label: "Light" },
                      { id: "dark", label: "Dark" },
                      { id: "auto", label: "Auto" },
                    ]}
                    value={theme}
                    onChange={(id) => onThemeChange(id as ThemePreference)}
                  />
                </SettingCard>

                <SettingCard label="Accent color">
                  <div
                    role="radiogroup"
                    aria-label="Accent color"
                    className="flex items-center gap-[11px]"
                  >
                    {ACCENT_SWATCHES.map((swatch) => {
                      const isSelected = swatch.id === "lime"
                      return (
                        <button
                          key={swatch.id}
                          type="button"
                          role="radio"
                          aria-checked={isSelected}
                          aria-label={swatch.id}
                          onClick={() => {
                            /* no-op placeholder — accent switching lands in a later task */
                          }}
                          style={{
                            backgroundColor: swatch.solid,
                            boxShadow: isSelected
                              ? `0 0 0 2px var(--paper-0), 0 0 0 4px ${swatch.solid}`
                              : "var(--shadow-xs)",
                          }}
                          className="flex size-[30px] shrink-0 items-center justify-center rounded-full transition-[transform,box-shadow] duration-150 ease-(--ease-spring) hover:scale-[1.12]"
                        >
                          {isSelected && (
                            <Check
                              className="size-[15px] text-white [&_path]:stroke-[3]"
                              aria-hidden="true"
                            />
                          )}
                        </button>
                      )
                    })}
                  </div>
                </SettingCard>

                <SettingCard label="Language">
                  <Segmented
                    label="Language"
                    options={[
                      { id: "en", label: "English" },
                      { id: "zh", label: "繁體中文" },
                    ]}
                    value={language}
                    onChange={setLanguage}
                  />
                </SettingCard>
              </div>
            )}

            {section === "editor" && (
              <div className="flex flex-col gap-[14px]">
                <SettingCard label="Editor font size" sub="Applied to the code editor">
                  <Segmented
                    label="Editor font size"
                    options={["12", "13", "14", "15"].map((size) => ({ id: size, label: size }))}
                    value={fontSize}
                    onChange={setFontSize}
                  />
                </SettingCard>

                {/* Format-on-save + the language-server list are owned by the LSP
                    pane (real, persisted / live) — the editor pane keeps only the
                    editor-surface toggle. */}
                <div className="flex flex-col">
                  <ToggleRow
                    label="Show minimap"
                    sub="Code overview strip on the editor edge"
                    checked={minimap}
                    onCheckedChange={setMinimap}
                  />
                </div>
              </div>
            )}

            {section === "safety" && (
              <div className="flex flex-col">
                <ToggleRow
                  label="Reconcile external changes"
                  sub="Reload files changed on disk by other tools"
                  checked={reconcile}
                  onCheckedChange={setReconcile}
                />
                <ToggleRow
                  label="Confirm destructive git actions"
                  sub="Require hold-to-confirm for discard & reset"
                  locked
                  checked={confirmGit}
                  onCheckedChange={setConfirmGit}
                />
              </div>
            )}

            {section === "lsp" && <LspSection targetLanguage={targetLanguage} />}

            {section === "agent" && <AgentSection />}

            {section === "logs" && (
              <LogsSection initialSource={settingsLogSource ?? undefined} openNonce={openNonce} />
            )}

            {section === "terminal" && <TerminalSection />}

            {section === "preview" && <PreviewSection />}

            {section === "git" && <GitSection />}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
