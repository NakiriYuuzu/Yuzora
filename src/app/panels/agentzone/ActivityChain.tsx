import { useState } from "react"
import { useTranslation } from "react-i18next"
import {
  BotIcon,
  BrainIcon,
  FileTextIcon,
  PencilIcon,
  SearchIcon,
  TerminalIcon,
  WrenchIcon,
  type LucideIcon,
} from "lucide-react"

import type { BlockEntry } from "@/agent/acpTypes"
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
  ChainOfThoughtSearchResult,
  ChainOfThoughtSearchResults,
  ChainOfThoughtStep,
} from "@/components/ai/chain-of-thought"

import { parseToolBlockMeta, ToolDetail, toolPreStyle } from "./blocks"
import { subagentInvocation, type SubagentInvocation } from "./subagent"

// 一條 activity 鏈＝segmentTranscript 聚合出的連續 tool／thought 群組（spec P1）。
// 串流中（live）自動展開、回合結束自動收合成一行 header；使用者點擊後以手動
// 狀態為準（隨時可重開）。
export function ActivityChain({ entries, live }: { entries: BlockEntry[]; live: boolean }) {
  const { t } = useTranslation("panels")
  const [userOpen, setUserOpen] = useState<boolean | null>(null)
  const open = userOpen ?? live
  const failed = entries.filter((entry) => parseToolBlockMeta(entry.meta).status === "failed").length
  return (
    <ChainOfThought
      open={open}
      onOpenChange={setUserOpen}
      data-testid="agent-activity-chain"
      // space-y-0：收合時 ChainOfThoughtContent 的空 Collapsible wrapper 仍在 DOM，
      // root 的 space-y 會對它加 margin 造成下方多餘留白；展開間距由 content 自身 mt-2 提供。
      className="max-w-none space-y-0 rounded-xl border border-(--line-1) bg-(--yz-glass) px-3 py-1.5 backdrop-blur-md"
    >
      <ChainOfThoughtHeader>
        <span>
          {live
            ? t("agentZonePanel.chainWorking", { n: entries.length })
            : t("agentZonePanel.chainSteps", { n: entries.length })}
        </span>
        {failed > 0 && (
          <span style={{ color: "var(--destructive)", marginLeft: 8, fontSize: "0.85em" }}>
            {t("agentZonePanel.chainFailed", { n: failed })}
          </span>
        )}
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent>
        {renderChainEntries(entries, live, t)}
      </ChainOfThoughtContent>
    </ChainOfThought>
  )
}

// sub-agent 嵌套歸組（claude：子 agent 內部 tool call 帶 parentToolCallId）：
// 有 parent 且 parent 在本 chain 內者，收進該 spawn step 底下縮排呈現；其餘
// 照原順序平鋪。pi/codex 的子活動不在 wire 上，天然只有頂層 spawn 卡片。
function renderChainEntries(
  entries: BlockEntry[],
  live: boolean,
  t: (key: string, options?: Record<string, unknown>) => string
) {
  const toolCallIds = new Set(
    entries.map((entry) => parseToolBlockMeta(entry.meta).toolCallId).filter(Boolean)
  )
  const childrenByParent = new Map<string, BlockEntry[]>()
  const topLevel: BlockEntry[] = []
  for (const entry of entries) {
    const parent = entry.kind === "tool" ? parseToolBlockMeta(entry.meta).parentToolCallId : undefined
    if (parent && toolCallIds.has(parent)) {
      const siblings = childrenByParent.get(parent) ?? []
      siblings.push(entry)
      childrenByParent.set(parent, siblings)
    } else {
      topLevel.push(entry)
    }
  }
  return topLevel.map((entry, index) => {
    if (entry.kind === "thought") {
      return (
        <ChainOfThoughtStep
          key={entry.id}
          icon={BrainIcon}
          label={t("agentZonePanel.chainThought")}
          description={entry.text}
          status={live && index === topLevel.length - 1 ? "active" : "complete"}
        />
      )
    }
    const meta = parseToolBlockMeta(entry.meta)
    const invocation = subagentInvocation(entry.text.split("\n")[0], meta.rawInput)
    if (invocation) {
      return (
        <SubagentStep
          key={entry.id}
          entry={entry}
          invocation={invocation}
          childEntries={meta.toolCallId ? childrenByParent.get(meta.toolCallId) ?? [] : []}
        />
      )
    }
    return <ToolStep key={entry.id} entry={entry} />
  })
}

// Sub-agent 專屬呈現（soak 回饋 2026-07-22）：agent type chip＋任務描述為主標，
// 展開見完整 prompt／結果；claude 的子 agent 內部工具鏈縮排嵌套在卡片下。
function SubagentStep({
  entry,
  invocation,
  childEntries,
}: {
  entry: BlockEntry
  invocation: SubagentInvocation
  childEntries: BlockEntry[]
}) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const meta = parseToolBlockMeta(entry.meta)
  const [title, ...rest] = entry.text.split("\n")
  const body = rest.join("\n")
  const task = invocation.task ?? title
  const label = (
    <span style={{ display: "inline-flex", alignItems: "baseline", gap: 7, minWidth: 0, maxWidth: "100%" }}>
      <span
        data-testid="subagent-chip"
        style={{
          flex: "0 0 auto",
          padding: "1px 7px",
          borderRadius: 999,
          background: "var(--agent-pi-soft, rgba(91,63,209,0.1))",
          border: "1px solid rgba(91,63,209,0.25)",
          color: "#5b3fd1",
          fontSize: "0.82em",
          fontWeight: 600,
          letterSpacing: "0.01em",
        }}
      >
        {invocation.kind === "manage"
          ? t("agentZonePanel.subagentManageChip")
          : invocation.agentType ?? t("agentZonePanel.subagentChip")}
      </span>
      <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
        {task}
      </span>
      {invocation.background && (
        <span style={{ flex: "0 0 auto", fontSize: "0.8em", color: "var(--ink-3)" }}>
          {t("agentZonePanel.subagentBackground")}
        </span>
      )}
      {invocation.model && (
        <span style={{ flex: "0 0 auto", fontFamily: "var(--font-mono)", fontSize: "0.8em", color: "var(--ink-4)" }}>
          {invocation.model}
        </span>
      )}
    </span>
  )
  return (
    <ChainOfThoughtStep
      data-testid="subagent-step"
      icon={BotIcon}
      status={stepStatus(meta.status)}
      label={
        <button
          type="button"
          aria-label={t("agentZonePanel.subagentToggle")}
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
          style={{
            background: "transparent",
            border: 0,
            padding: 0,
            cursor: "pointer",
            font: "inherit",
            color: "inherit",
            textAlign: "left",
            maxWidth: "100%",
          }}
        >
          {label}
        </button>
      }
    >
      {open && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {invocation.prompt && (
            <pre style={toolPreStyle}>{invocation.prompt}</pre>
          )}
          {body && <pre style={toolPreStyle}>{body}</pre>}
          {meta.rawOutput && <ToolDetail label={t("agentZonePanel.subagentResult")} value={meta.rawOutput} />}
        </div>
      )}
      {childEntries.length > 0 && (
        <div
          data-testid="subagent-children"
          style={{
            marginTop: 4,
            paddingLeft: 10,
            borderLeft: "2px solid rgba(91,63,209,0.25)",
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          {childEntries.map((child) => (
            <ToolStep key={child.id} entry={child} />
          ))}
        </div>
      )}
    </ChainOfThoughtStep>
  )
}

function ToolStep({ entry }: { entry: BlockEntry }) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const meta = parseToolBlockMeta(entry.meta)
  const [title, ...rest] = entry.text.split("\n")
  const body = rest.join("\n")
  const hasDetail = Boolean(body || meta.rawInput || meta.rawOutput)
  // 2026-07-21 使用者回饋：step 標題附上實際調用內容（bash 指令、read/write
  // 檔案⋯）。多數 adapter 的 title 只有 tool 名，內容藏在 rawInput；title 已含
  // 內容者（如自組字串的 adapter）不重複顯示。
  const invocation = toolInvocationDetail(meta.rawInput)
  const showInvocation = invocation !== undefined && !title.includes(invocation)
  const labelText = (
    <span style={{ display: "inline-flex", alignItems: "baseline", gap: 7, minWidth: 0, maxWidth: "100%" }}>
      <span style={{ flex: "0 0 auto" }}>{title}</span>
      {showInvocation && (
        <span
          style={{
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            fontFamily: "var(--font-mono)",
            fontSize: "0.92em",
            color: "var(--ink-3)",
          }}
        >
          {invocation}
        </span>
      )}
    </span>
  )
  return (
    <ChainOfThoughtStep
      icon={stepIcon(metaKind(entry.meta))}
      status={stepStatus(meta.status)}
      label={
        hasDetail ? (
          <button
            type="button"
            aria-label={t("agentZonePanel.toolToggle")}
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
            style={{
              background: "transparent",
              border: 0,
              padding: 0,
              cursor: "pointer",
              font: "inherit",
              color: "inherit",
              textAlign: "left",
              maxWidth: "100%",
            }}
          >
            {labelText}
          </button>
        ) : (
          labelText
        )
      }
    >
      {meta.locations && meta.locations.length > 0 && (
        <ChainOfThoughtSearchResults>
          {meta.locations.map((location, index) => (
            <ChainOfThoughtSearchResult key={`${location.path}-${index}`}>
              {location.line != null ? `${location.path}:${location.line}` : location.path}
            </ChainOfThoughtSearchResult>
          ))}
        </ChainOfThoughtSearchResults>
      )}
      {open && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {body && <pre style={toolPreStyle}>{body}</pre>}
          {meta.rawInput && <ToolDetail label={t("agentZonePanel.toolInput")} value={meta.rawInput} />}
          {meta.rawOutput && <ToolDetail label={t("agentZonePanel.toolOutput")} value={meta.rawOutput} />}
        </div>
      )}
    </ChainOfThoughtStep>
  )
}

// rawInput 中最能代表「這次調用做了什麼」的欄位（跨 adapter 的常見鍵名）：
// bash 的 command、read/write/edit 的 path、search 的 pattern⋯。取第一個命中者。
const INVOCATION_KEYS = [
  "command",
  "cmd",
  "path",
  "file_path",
  "filePath",
  "pattern",
  "query",
  "url",
] as const

export function toolInvocationDetail(rawInput?: Record<string, unknown>): string | undefined {
  if (!rawInput) return undefined
  for (const key of INVOCATION_KEYS) {
    const value = rawInput[key]
    if (typeof value === "string" && value.trim() !== "") return value
    if (Array.isArray(value) && value.length > 0 && value.every((item) => typeof item === "string")) {
      return value.join(" ")
    }
  }
  return undefined
}

// ACP tool kind（read/edit/delete/move/search/execute/think/fetch/other）→ step 圖示。
function stepIcon(kind: string | undefined): LucideIcon {
  switch (kind) {
    case "read":
    case "fetch":
      return FileTextIcon
    case "edit":
    case "delete":
    case "move":
      return PencilIcon
    case "execute":
      return TerminalIcon
    case "search":
      return SearchIcon
    default:
      return WrenchIcon
  }
}

// ACP status 是不透明字串：pending/in_progress→active、failed→failed、其餘（含
// completed 與未知值）→complete（spec「聚合模型」的有損對照）。
function stepStatus(status: string | undefined): "complete" | "active" | "failed" {
  if (status === "pending" || status === "in_progress") return "active"
  if (status === "failed") return "failed"
  return "complete"
}

function metaKind(meta: string | undefined): string | undefined {
  if (!meta) return undefined
  try {
    const parsed = JSON.parse(meta) as Record<string, unknown>
    return typeof parsed.kind === "string" ? parsed.kind : undefined
  } catch {
    return undefined
  }
}
