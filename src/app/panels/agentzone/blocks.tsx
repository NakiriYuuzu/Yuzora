import { useState, type CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import type { BlockEntry, TranscriptAction } from "@/agent/acpTypes"
import type { AgentId } from "@/lib/agentPresets"
import { useAgentStore } from "@/state/agentStore"

type ThreadKind = BlockEntry["kind"]

const THREAD_KIND_STYLE: Record<ThreadKind, {
  bg: string
  bd: string
  fg: string
  accent: string
}> = {
  tool: {
    bg: "var(--paper-1)",
    bd: "var(--line-1)",
    fg: "var(--ink-2)",
    accent: "var(--yz-accent-ink)",
  },
  diff: {
    bg: "var(--mint-soft)",
    bd: "rgba(43,191,138,0.28)",
    fg: "#0f7a55",
    accent: "#0f7a55",
  },
  perm: {
    bg: "var(--amber-soft)",
    bd: "rgba(214,138,12,0.3)",
    fg: "#9a6512",
    accent: "#9a6512",
  },
  error: {
    bg: "var(--danger-soft)",
    bd: "rgba(226,59,84,0.3)",
    fg: "#b51f38",
    accent: "#c2293f",
  },
  plan: {
    bg: "var(--paper-1)",
    bd: "var(--line-1)",
    fg: "var(--ink-2)",
    accent: "#5b3fd1",
  },
  thought: {
    bg: "var(--paper-2)",
    bd: "var(--line-1)",
    fg: "var(--ink-3)",
    accent: "var(--ink-4)",
  },
  notice: {
    bg: "var(--amber-soft)",
    bd: "rgba(214,138,12,0.3)",
    fg: "#9a6512",
    accent: "#9a6512",
  },
}

export interface ToolBlockMeta {
  status?: string
  rawInput?: Record<string, unknown>
  rawOutput?: Record<string, unknown>
  locations?: { path: string; line?: number | null }[]
  /** connection 層的 tool call id（sub-agent 嵌套歸組的 key）。 */
  toolCallId?: string
  /** sub-agent 內部 tool call → spawn 它的 tool call（claude parentToolUseId）。 */
  parentToolCallId?: string
}

export function parseToolBlockMeta(meta?: string): ToolBlockMeta {
  if (!meta) return {}
  try {
    const p = JSON.parse(meta) as Record<string, unknown>
    return {
      status: typeof p.status === "string" ? p.status : undefined,
      rawInput: p.rawInput && typeof p.rawInput === "object" ? p.rawInput as Record<string, unknown> : undefined,
      rawOutput: p.rawOutput && typeof p.rawOutput === "object" ? p.rawOutput as Record<string, unknown> : undefined,
      locations: Array.isArray(p.locations) ? p.locations as ToolBlockMeta["locations"] : undefined,
      toolCallId: typeof p.toolCallId === "string" ? p.toolCallId : undefined,
      parentToolCallId: typeof p.parentToolCallId === "string" ? p.parentToolCallId : undefined,
    }
  } catch { return {} }
}

export function ToolBlock({ entry }: { entry: BlockEntry }) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const meta = parseToolBlockMeta(entry.meta)
  const style = meta.status === "failed" ? THREAD_KIND_STYLE.error : THREAD_KIND_STYLE.tool
  const [title, ...rest] = entry.text.split("\n")
  const body = rest.join("\n")
  const preview = body.length > 50 ? `${body.slice(0, 50)}…` : body
  return (
    <div style={{ display: "flex" }}>
      <div style={{ flex: 1, minWidth: 0, borderRadius: 12, background: style.bg, border: `1px solid ${style.bd}`, display: "flex", flexDirection: "column" }}>
        <button type="button" aria-label={t("agentZonePanel.toolToggle")} aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          style={{ display: "flex", alignItems: "center", gap: 10, padding: "10px 12px", background: "transparent", border: 0, cursor: "pointer", textAlign: "left", minWidth: 0 }}>
          <span aria-hidden="true" style={{ width: 3, alignSelf: "stretch", borderRadius: 3, background: style.accent, flex: "0 0 auto" }} />
          <span style={{ flex: 1, minWidth: 0, fontFamily: "var(--font-mono)", fontSize: 11.5, fontWeight: 500, color: style.fg, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {title}{preview ? ` · ${preview}` : ""}
          </span>
          {meta.status && <span style={{ fontFamily: "var(--font-mono)", fontSize: 10.5, color: "var(--ink-3)", flex: "0 0 auto" }}>{meta.status}</span>}
        </button>
        {open && (
          <div style={{ padding: "0 12px 12px 25px", display: "flex", flexDirection: "column", gap: 8 }}>
            {body && <pre style={toolPreStyle}>{body}</pre>}
            {meta.rawInput && <ToolDetail label={t("agentZonePanel.toolInput")} value={meta.rawInput} />}
            {meta.rawOutput && <ToolDetail label={t("agentZonePanel.toolOutput")} value={meta.rawOutput} />}
            {meta.locations && meta.locations.length > 0 && (
              <ToolDetail label={t("agentZonePanel.toolLocations")}
                value={meta.locations.map((l) => (l.line != null ? `${l.path}:${l.line}` : l.path))} />
            )}
          </div>
        )}
      </div>
    </div>
  )
}
export const toolPreStyle: CSSProperties = { margin: 0, fontFamily: "var(--font-mono)", fontSize: 11, lineHeight: 1.5, color: "var(--ink-2)", whiteSpace: "pre-wrap", wordBreak: "break-word" }
export function ToolDetail({ label, value }: { label: string; value: unknown }) {
  return (
    <div>
      <div style={{ fontSize: 9.5, fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.04em", color: "var(--ink-3)", marginBottom: 3 }}>{label}</div>
      <pre style={toolPreStyle}>{JSON.stringify(value, null, 2)}</pre>
    </div>
  )
}

export function TranscriptBlock({ entry, sessionId }: { entry: BlockEntry; sessionId: string }) {
  const { t } = useTranslation("panels")
  const respondPermission = useAgentStore((s) => s.respondPermission)
  const newSession = useAgentStore((s) => s.newSession)
  // P3：perm 卡答覆後鎖定按鈕並顯示所選結果（respondPermission 記錄的 outcome）。
  const permOutcome = useAgentStore((s) =>
    entry.kind === "perm" ? s.sessions.get(sessionId)?.permissionOutcomes?.[entry.id] : undefined
  )
  const answered = permOutcome !== undefined
  const pickedLabel = answered
    ? entry.actions?.find((action) => optionIdFromAction(action) === permOutcome)?.label ?? permOutcome
    : undefined
  const style = THREAD_KIND_STYLE[entry.kind] ?? THREAD_KIND_STYLE.tool
  const isMono = entry.kind === "tool"

  function onAction(action: TranscriptAction) {
    if (entry.kind === "notice" && action.kind === "start_new_session") {
      const payload = newSessionPayloadFromAction(action)
      if (payload) void newSession(payload.cwd, payload.agentId).catch(() => undefined)
      return
    }
    if (entry.kind !== "perm") return
    const optionId = optionIdFromAction(action)
    if (optionId) respondPermission(sessionId, optionId)
  }

  return (
    <div style={{ display: "flex" }}>
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 12px",
          borderRadius: 12,
          background: "var(--yz-glass)",
          backdropFilter: "blur(12px)",
          WebkitBackdropFilter: "blur(12px)",
          border: `1px solid ${style.bd}`,
          minWidth: 0,
        }}
      >
        <span
          aria-hidden="true"
          style={{
            width: 3,
            alignSelf: "stretch",
            borderRadius: 3,
            flex: "0 0 auto",
            background: style.accent,
          }}
        />
        <span
          style={{
            flex: 1,
            minWidth: 0,
            fontSize: isMono ? 11.5 : 12,
            fontWeight: 500,
            color: style.fg,
            fontFamily: isMono ? "var(--font-mono)" : undefined,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "pre-wrap",
          }}
        >
          {entry.text}
        </span>
        {(entry.meta || answered) && (
          <span
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: 10.5,
              color: answered ? "var(--yz-accent-ink)" : "var(--ink-3)",
              flex: "0 0 auto",
            }}
          >
            {answered ? t("agentZonePanel.permAnswered", { option: pickedLabel }) : displayMeta(entry)}
          </span>
        )}
        {entry.actions && entry.actions.length > 0 && (
          <div style={{ display: "flex", gap: 7, flex: "0 0 auto" }}>
            {entry.actions.map((action, index) => {
              const picked = answered && optionIdFromAction(action) === permOutcome
              return (
                <button
                  key={`${action.label}-${index}`}
                  type="button"
                  disabled={answered}
                  aria-pressed={entry.kind === "perm" ? picked : undefined}
                  onClick={() => onAction(action)}
                  style={{
                    ...actionChipStyle(index === 0, style.accent),
                    ...(answered
                      ? picked
                        ? { outline: "1.5px solid var(--yz-accent-ink)", cursor: "default" }
                        : { opacity: 0.45, cursor: "default" }
                      : {}),
                  }}
                >
                  {action.label}
                  {picked ? " ✓" : ""}
                </button>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}

function actionChipStyle(primary: boolean, accent: string): CSSProperties {
  return {
    display: "inline-flex",
    alignItems: "center",
    height: 26,
    padding: "0 12px",
    borderRadius: 999,
    fontSize: 11,
    fontWeight: 600,
    cursor: "pointer",
    flex: "0 0 auto",
    border: primary ? "none" : "1px solid var(--line-1)",
    background: primary ? accent : "var(--yz-solid)",
    color: primary ? "#fff" : "var(--ink-1)",
    boxShadow: primary ? "var(--shadow-xs)" : undefined,
  }
}

function optionIdFromAction(action: TranscriptAction): string | null {
  const payload = action.payload
  if (payload && typeof payload === "object" && "optionId" in payload) {
    const optionId = (payload as { optionId?: unknown }).optionId
    return typeof optionId === "string" ? optionId : null
  }
  return action.kind || null
}

function newSessionPayloadFromAction(action: TranscriptAction): { cwd: string; agentId?: AgentId } | null {
  const payload = action.payload
  if (!payload || typeof payload !== "object") return null
  const record = payload as Record<string, unknown>
  if (typeof record.cwd !== "string") return null
  const agentId = record.agentId
  return {
    cwd: record.cwd,
    ...(agentId === "pi" || agentId === "claude" || agentId === "codex" ? { agentId } : {})
  }
}

// plan 專屬 frost 卡（spec P2）：把 upsertPlan 壓平的 [x]/[wip]/[] 文字清單
// 渲染成 checkbox 清單；無標記的行（外部 agent 自帶符號）原樣顯示。
export function PlanBlock({ entry }: { entry: BlockEntry }) {
  const { t } = useTranslation("panels")
  const lines = entry.text.split("\n").filter((line) => line.trim() !== "")
  return (
    <div
      data-testid="agent-plan-block"
      style={{
        borderRadius: 12,
        background: "var(--yz-glass)",
        backdropFilter: "blur(12px)",
        WebkitBackdropFilter: "blur(12px)",
        border: "1px solid var(--line-1)",
        padding: "9px 12px",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
        <span
          aria-hidden="true"
          style={{
            width: 18,
            height: 18,
            borderRadius: 6,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 10,
            flex: "0 0 auto",
            background: "rgba(91,63,209,0.10)",
            color: "#5b3fd1",
          }}
        >
          ☰
        </span>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, fontWeight: 600, color: "var(--ink-1)" }}>
          {t("agentZonePanel.planTitle")}
        </span>
        {entry.meta && (
          <span style={{ marginLeft: "auto", fontFamily: "var(--font-mono)", fontSize: 9.5, color: "var(--ink-3)" }}>
            {displayMeta(entry)}
          </span>
        )}
      </div>
      <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2, fontSize: 12 }}>
        {lines.map((line, index) => (
          <PlanItem key={index} line={line} />
        ))}
      </ul>
    </div>
  )
}

function PlanItem({ line }: { line: string }) {
  const parsed = /^\[(x|wip|)\]\s*(.*)$/.exec(line)
  if (!parsed) {
    return <li style={{ color: "var(--ink-2)" }}>{line}</li>
  }
  const state = parsed[1]
  const done = state === "x"
  const wip = state === "wip"
  return (
    <li style={{ display: "flex", alignItems: "center", gap: 7, color: done ? "var(--ink-4)" : wip ? "var(--ink-0)" : "var(--ink-2)" }}>
      <span
        aria-hidden="true"
        style={{
          width: 12,
          height: 12,
          borderRadius: 4,
          flex: "0 0 auto",
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 8,
          border: `1.5px solid ${done || wip ? "var(--yz-accent)" : "var(--ink-4)"}`,
          background: done ? "var(--yz-accent)" : "transparent",
          color: done ? "#223005" : "var(--yz-accent-ink)",
        }}
      >
        {done ? "✓" : wip ? "▸" : ""}
      </span>
      <span style={{ minWidth: 0, fontWeight: wip ? 600 : undefined }}>{parsed[2]}</span>
    </li>
  )
}

function displayMeta(entry: BlockEntry): string {
  if (!entry.meta) return ""
  try {
    const parsed = JSON.parse(entry.meta) as Record<string, unknown>
    if (entry.kind === "plan" && typeof parsed.completed === "number" && typeof parsed.total === "number") {
      return `${parsed.completed}/${parsed.total}`
    }
    if (typeof parsed.status === "string") return parsed.status
    if (typeof parsed.kind === "string") return parsed.kind
    if (typeof parsed.toolCallId === "string") return parsed.toolCallId
  } catch {
    return entry.meta
  }
  return entry.meta
}
