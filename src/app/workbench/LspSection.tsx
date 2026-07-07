import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { Download, FileText, RefreshCw, Trash2 } from "lucide-react"

import {
  lspConfigClearStale,
  lspConfigGet,
  lspConfigSetServer,
  lspConfigStale,
  lspInstallServer,
  lspSetTrace,
  lspStatus,
} from "@/lib/ipc"
import type {
  LspConfig,
  LspInstallProgress,
  LspLanguage,
  LspServerInfo,
} from "@/lib/types"
import { cn } from "@/lib/utils"
import { FORMAT_ON_SAVE_STORAGE_KEY } from "@/editor/EditorPane"
import { useLspStore } from "@/state/lspStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { SettingCard, ToggleRow } from "./settingsPrimitives"

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
export function LspSection({ targetLanguage }: { targetLanguage?: string }) {
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
