import {
  Bot,
  SendHorizontal,
  Slash,
  Square,
  X,
  type LucideIcon,
} from "lucide-react"
import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
} from "react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import type { PromptBlock, SlashCommand } from "@/agent/acpConnection"
import type { BlockEntry, TranscriptAction, TranscriptEntry } from "@/agent/acpTypes"
import { firstAbsolutePath } from "@/lib/paths"
import { pathToUri } from "@/lsp/workspace"
import { contextMenuHandler } from "@/state/contextMenuStore"
import type { AgentTone, SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

type AgentVisual = {
  label: string
  short: string
  color: string
  bg: string
}

type ToneVisual = {
  label: string
  dot: string
  bg: string
  fg: string
  pulse?: boolean
}

type ThreadKind = BlockEntry["kind"]

interface AgentDiffPayload {
  path: string
  oldText: string | null
  newText: string
}

const AGENT_TYPES: Record<string, AgentVisual> = {
  claude: { label: "Claude", short: "C", color: "#c0562f", bg: "rgba(192,86,47,0.12)" },
  codex: { label: "Codex", short: "X", color: "#0f7a5f", bg: "rgba(15,122,95,0.12)" },
  pi: { label: "Pi", short: "π", color: "#6d4dd6", bg: "rgba(109,77,214,0.12)" },
}

// Tone label text is localized at the call site (ActiveAgentSession) via the
// component's `t`; this map only carries the tone's non-text styling.
const TONE_STYLE: Record<AgentTone, Omit<ToneVisual, "label">> = {
  idle: { dot: "var(--ink-4)", bg: "var(--paper-2)", fg: "var(--ink-3)" },
  run: {
    dot: "var(--yz-accent)",
    bg: "#eef6d6",
    fg: "var(--yz-accent-ink)",
    pulse: true,
  },
  done: { dot: "#2bbf8a", bg: "var(--mint-soft)", fg: "#0f7a55" },
  wait: { dot: "#ffb23e", bg: "var(--amber-soft)", fg: "#9a6512" },
  fail: { dot: "#e23b54", bg: "var(--danger-soft)", fg: "#b51f38" },
}

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
}

function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path
}

/**
 * AgentZone session surface. Active-session layout mirrors dc.html L940-L1007;
 * style constants are the expanded values from L3126-L3195 plus toneMap
 * L3080-L3085. Empty state stays intact for the existing entry-state tests.
 */
export function AgentZonePanel() {
  const activeSessionId = useAgentStore((s) => s.activeSessionId)
  const session = useAgentStore((s) =>
    activeSessionId ? (s.sessions.get(activeSessionId) ?? null) : null
  )
  const authRequired = useAgentStore((s) => s.authRequired)
  const connectionState = useAgentStore((s) => s.connectionState)
  const connectionError = useAgentStore((s) => s.connectionError)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const showConnectionError = !authRequired
    && (connectionState === "error" || connectionError !== null)
  // cwd 防呆：沒有絕對路徑的 workspace／session cwd 時顯示引導，避免以相對路徑 spawn。
  const showWorkspaceGuide = !authRequired
    && !showConnectionError
    && firstAbsolutePath(workspacePath, session?.cwd) === null

  return (
    <div
      onContextMenu={contextMenuHandler("agent")}
      className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)"
    >
      {authRequired && <AuthRequiredBanner />}
      {showConnectionError && <ConnectionErrorBanner />}
      {showWorkspaceGuide && <WorkspaceGuideBanner />}
      {activeSessionId && session ? (
        <ActiveAgentSession sessionId={activeSessionId} session={session} />
      ) : (
        <AgentEmptyState />
      )}
    </div>
  )
}

function AuthRequiredBanner() {
  const { t } = useTranslation("panels")
  const beginTerminalLogin = useAgentStore((s) => s.beginTerminalLogin)
  const retryAfterLogin = useAgentStore((s) => s.retryAfterLogin)

  return (
    <div className="flex shrink-0 items-center gap-[10px] border-b border-[rgba(214,138,12,0.3)] bg-(--amber-soft) px-[14px] py-[10px] text-[#9a6512]">
      <Bot className="size-[15px] shrink-0" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <div className="text-[12.5px] font-semibold">{t("agentZonePanel.authRequiredTitle")}</div>
        <div className="truncate text-[11px] text-[#9a6512]/80">
          {t("agentZonePanel.authRequiredDescription")}
        </div>
      </div>
      <button
        type="button"
        onClick={beginTerminalLogin}
        className="flex h-[28px] shrink-0 items-center rounded-[8px] bg-[#9a6512] px-[11px] text-[11.5px] font-semibold text-white shadow-(--shadow-xs)"
      >
        {t("agentZonePanel.openTerminalLogin")}
      </button>
      <button
        type="button"
        onClick={() => void retryAfterLogin().catch(() => undefined)}
        className="flex h-[28px] shrink-0 items-center rounded-[8px] border border-[rgba(154,101,18,0.26)] bg-(--paper-0) px-[11px] text-[11.5px] font-semibold text-[#9a6512]"
      >
        {t("agentZonePanel.retry")}
      </button>
    </div>
  )
}

function ConnectionErrorBanner() {
  const { t } = useTranslation("panels")
  const connectionError = useAgentStore((s) => s.connectionError)
  const activeSessionId = useAgentStore((s) => s.activeSessionId)
  const session = useAgentStore((s) =>
    activeSessionId ? (s.sessions.get(activeSessionId) ?? null) : null
  )
  const newSession = useAgentStore((s) => s.newSession)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  function retry() {
    const cwd = firstAbsolutePath(workspacePath, session?.cwd)
    if (!cwd) return
    void newSession(cwd).catch(() => undefined)
  }

  return (
    <div className="flex shrink-0 items-center gap-[10px] border-b border-[rgba(226,59,84,0.3)] bg-(--danger-soft) px-[14px] py-[10px] text-[#b51f38]">
      <Bot className="size-[15px] shrink-0" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <div className="text-[12.5px] font-semibold">{t("agentZonePanel.connectionErrorTitle")}</div>
        <div className="truncate text-[11px] text-[#b51f38]/80">
          {connectionError ?? t("agentZonePanel.unknownError")}
        </div>
      </div>
      <button
        type="button"
        onClick={retry}
        className="flex h-[28px] shrink-0 items-center rounded-[8px] bg-[#b51f38] px-[11px] text-[11.5px] font-semibold text-white shadow-(--shadow-xs)"
      >
        {t("agentZonePanel.retry")}
      </button>
    </div>
  )
}

function WorkspaceGuideBanner() {
  const { t } = useTranslation("panels")
  return (
    <div className="flex shrink-0 items-center gap-[10px] border-b border-(--line-1) bg-(--paper-1) px-[14px] py-[10px] text-(--ink-2)">
      <Bot className="size-[15px] shrink-0" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <div className="text-[12.5px] font-semibold">{t("agentZonePanel.noWorkspaceTitle")}</div>
        <div className="truncate text-[11px] text-(--ink-3)">
          {t("agentZonePanel.noWorkspaceDescription")}
        </div>
      </div>
    </div>
  )
}

function AgentEmptyState() {
  const { t } = useTranslation("panels")
  return (
    <>
      <div className="flex min-h-0 flex-1 items-center justify-center">
        <EmptyState
          icon={Bot}
          title={t("agentZonePanel.emptyTitle")}
          description={t("agentZonePanel.emptyDescription")}
        />
      </div>

      <div className="flex shrink-0 items-center gap-[8px] border-t border-(--line-1) px-[14px] py-[11px]">
        <textarea
          disabled
          rows={1}
          aria-label={t("agentZonePanel.messageAriaLabel")}
          placeholder={t("agentZonePanel.messagePlaceholder")}
          className="h-[38px] min-w-0 flex-1 resize-none rounded-[10px] border border-[#7b5bff]/30 bg-(--yz-field) px-[12px] py-[9px] text-[13px] text-(--ink-3) placeholder:text-(--ink-4) disabled:cursor-not-allowed"
        />
        <button
          type="button"
          disabled
          aria-label={t("agentZonePanel.sendAriaLabel")}
          className="flex size-[38px] shrink-0 items-center justify-center rounded-[10px] bg-(--ink-1) text-(--paper-0) disabled:cursor-not-allowed disabled:opacity-50"
        >
          <SendHorizontal className="size-[16px]" aria-hidden="true" />
        </button>
      </div>
    </>
  )
}

function ActiveAgentSession({
  sessionId,
  session,
}: {
  sessionId: string
  session: SessionState
}) {
  const { t } = useTranslation("panels")
  const [composer, setComposer] = useState("")
  const [slashOpen, setSlashOpen] = useState(false)
  const [slashIndex, setSlashIndex] = useState(0)
  const filterCommands = useAgentStore((s) => s.filterCommands)
  const sendPrompt = useAgentStore((s) => s.sendPrompt)
  const cancel = useAgentStore((s) => s.cancel)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const activeFilePath = useWorkspaceStore((s) => {
    const path = s.groups[s.activeGroupIndex]?.activePath ?? null
    return path === PREVIEW_TAB_PATH ? null : path
  })
  const [removedFilePath, setRemovedFilePath] = useState<string | null>(null)

  const agent = agentVisual(session.agentLabel, t("agentZonePanel.agentFallback"))
  const tone: ToneVisual = { ...TONE_STYLE[session.tone], label: t(`agentZonePanel.tone.${session.tone}`) }
  const commands = useMemo(
    () => (slashOpen ? filterCommands(composer, sessionId) : []),
    [composer, filterCommands, sessionId, slashOpen]
  )
  const selectedSlashIndex = Math.min(slashIndex, Math.max(0, commands.length - 1))
  const turnInProgress = session.tone === "run" || session.pendingTurn || session.running === true
  const attachedFilePath = activeFilePath && activeFilePath !== removedFilePath ? activeFilePath : null
  const attachedFileName = attachedFilePath ? fileNameFromPath(attachedFilePath) : null

  useEffect(() => {
    if (activeFilePath && removedFilePath && activeFilePath !== removedFilePath) {
      setRemovedFilePath(null)
    }
  }, [activeFilePath, removedFilePath])

  function openSlash() {
    setComposer((value) => (value.startsWith("/") ? value : "/"))
    setSlashOpen((value) => !value)
    setSlashIndex(0)
  }

  function updateComposer(value: string) {
    setComposer(value)
    setSlashOpen(value.startsWith("/"))
    setSlashIndex(0)
  }

  function pickSlash(command: SlashCommand) {
    setComposer(`/${command.name} `)
    setSlashOpen(false)
    setSlashIndex(0)
  }

  function submitPrompt() {
    if (turnInProgress) return
    const prompt = composer.trim()
    if (!prompt) return
    // cwd 防呆：沒有絕對路徑就不 spawn（引導訊息由 WorkspaceGuideBanner 顯示）。
    const cwd = firstAbsolutePath(workspacePath, session.cwd)
    if (!cwd) return
    const promptPayload: string | PromptBlock[] = attachedFilePath && attachedFileName
      ? [
          { type: "text", text: prompt },
          { type: "resource_link", uri: pathToUri(attachedFilePath), name: attachedFileName },
        ]
      : prompt
    setComposer("")
    setSlashOpen(false)
    setSlashIndex(0)
    void sendPrompt(cwd, promptPayload).catch(() => undefined)
  }

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (slashOpen) {
      if (event.key === "ArrowDown") {
        event.preventDefault()
        setSlashIndex((value) => Math.min(commands.length - 1, value + 1))
        return
      }
      if (event.key === "ArrowUp") {
        event.preventDefault()
        setSlashIndex((value) => Math.max(0, value - 1))
        return
      }
      if (event.key === "Escape") {
        event.preventDefault()
        setSlashOpen(false)
        return
      }
      if ((event.key === "Enter" || event.key === "Tab") && commands.length > 0) {
        event.preventDefault()
        pickSlash(commands[selectedSlashIndex] ?? commands[0])
        return
      }
    }

    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      submitPrompt()
    }
  }

  return (
    <>
      <SessionHeader session={session} agent={agent} tone={tone} />
      <TranscriptList sessionId={sessionId} session={session} agent={agent} />
      <div
        style={{
          position: "relative",
          borderTop: "1px solid var(--line-1)",
          background: "var(--paper-0)",
        }}
      >
        {slashOpen && (
          <SlashPopup
            commands={commands}
            selectedIndex={selectedSlashIndex}
            onPick={pickSlash}
          />
        )}
        {attachedFilePath && attachedFileName && (
          <div style={{ display: "flex", padding: "10px 14px 0" }}>
            <div
              title={attachedFilePath}
              style={{
                display: "inline-flex",
                alignItems: "center",
                minWidth: 0,
                maxWidth: "100%",
                height: 25,
                gap: 7,
                padding: "0 7px 0 10px",
                borderRadius: 999,
                background: "var(--yz-field)",
                border: "1px solid var(--line-2)",
                color: "var(--ink-2)",
                boxShadow: "var(--shadow-xs)",
              }}
            >
              <span
                style={{
                  minWidth: 0,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                  fontSize: 11.5,
                  fontWeight: 600,
                }}
              >
                {attachedFileName}
              </span>
              <button
                type="button"
                aria-label={t("agentZonePanel.removeFileContext", { fileName: attachedFileName })}
                onClick={() => setRemovedFilePath(attachedFilePath)}
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  justifyContent: "center",
                  width: 16,
                  height: 16,
                  flex: "0 0 auto",
                  border: 0,
                  borderRadius: 8,
                  padding: 0,
                  background: "rgba(27,26,23,0.06)",
                  color: "var(--ink-3)",
                  cursor: "pointer",
                }}
              >
                <X className="size-[11px]" aria-hidden="true" />
              </button>
            </div>
          </div>
        )}
        <div style={{ display: "flex", alignItems: "center", gap: 9, padding: "11px 14px" }}>
          <IconButton
            label={t("agentZonePanel.slashCommands")}
            title={t("agentZonePanel.slashCommandsTitle")}
            icon={Slash}
            onClick={openSlash}
            style={{
              width: 38,
              height: 38,
              borderRadius: 11,
              background: "var(--yz-field)",
              border: "1px solid var(--line-2)",
              color: slashOpen ? "#5b3fd1" : "var(--ink-2)",
              boxShadow: "var(--shadow-xs)",
            }}
          />

          <div
            style={{
              flex: 1,
              display: "flex",
              alignItems: "center",
              gap: 9,
              height: 38,
              padding: "0 13px",
              background: "var(--yz-field)",
              border: "1px solid var(--line-2)",
              borderRadius: 12,
              color: "var(--ink-3)",
              boxShadow: "var(--shadow-xs)",
            }}
          >
            <Bot className="size-[14px] shrink-0" aria-hidden="true" />
            <textarea
              rows={1}
              value={composer}
              aria-label={t("agentZonePanel.composerAriaLabel")}
              onChange={(event) => updateComposer(event.target.value)}
              onKeyDown={onComposerKeyDown}
              placeholder={t("agentZonePanel.replyPlaceholder", { agent: agent.label })}
              className="h-[36px] min-w-0 flex-1 resize-none bg-transparent pt-[9px] text-[13px] text-(--ink-1) outline-none placeholder:text-(--ink-4)"
            />
            <span
              aria-hidden="true"
              style={{ fontFamily: "var(--font-mono)", fontSize: 10, color: "var(--ink-4)" }}
            >
              ⌘↵
            </span>
          </div>

          {turnInProgress ? (
            <IconButton
              label={t("agentZonePanel.cancelAriaLabel")}
              title={t("agentZonePanel.cancel")}
              icon={Square}
              onClick={() => cancel(sessionId)}
              style={{
                width: 38,
                height: 38,
                borderRadius: 11,
                background: "var(--danger-soft)",
                color: "#b51f38",
                boxShadow: "var(--shadow-xs)",
              }}
            />
          ) : (
            <IconButton
              label={t("agentZonePanel.sendAriaLabel")}
              title={t("agentZonePanel.send")}
              icon={SendHorizontal}
              onClick={submitPrompt}
              disabled={!composer.trim()}
              style={{
                width: 38,
                height: 38,
                borderRadius: 11,
                background: "var(--ink-1)",
                color: "var(--paper-0)",
                boxShadow: "var(--shadow-xs)",
              }}
            />
          )}
        </div>
      </div>
    </>
  )
}

function SessionHeader({
  session,
  agent,
  tone,
}: {
  session: SessionState
  agent: AgentVisual
  tone: ToneVisual
}) {
  const { t } = useTranslation("panels")
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 11,
        padding: "13px 16px",
        borderBottom: "1px solid var(--line-1)",
        background: `linear-gradient(90deg, ${hexAlpha(agent.color, 0.13)}, transparent 70%)`,
      }}
    >
      <span
        aria-hidden="true"
        style={{
          width: 30,
          height: 30,
          borderRadius: 9,
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 13,
          fontWeight: 700,
          color: "#fff",
          flex: "0 0 auto",
          background: agent.color,
        }}
      >
        {agent.short}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontFamily: "var(--font-serif)",
            fontSize: 16,
            fontWeight: 600,
            color: "var(--ink-0)",
            letterSpacing: "-0.01em",
            lineHeight: 1.15,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {session.title}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 3 }}>
          <span
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 5,
              height: 20,
              padding: "0 9px 0 4px",
              borderRadius: "var(--r-pill)",
              fontSize: 10.5,
              fontWeight: 600,
              flex: "0 0 auto",
              background: agent.bg,
              color: agent.color,
            }}
          >
            <span
              aria-hidden="true"
              style={{
                width: 14,
                height: 14,
                borderRadius: 4,
                display: "inline-flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 8,
                fontWeight: 700,
                color: "#fff",
                flex: "0 0 auto",
                background: agent.color,
              }}
            >
              {agent.short}
            </span>
            {agent.label}
          </span>
          <span
            style={{
              display: "inline-flex",
              alignItems: "center",
              height: 18,
              padding: "0 7px",
              borderRadius: "var(--r-pill)",
              fontFamily: "var(--font-mono)",
              fontSize: 9.5,
              fontWeight: 700,
              background: "rgba(91,63,209,0.13)",
              color: "#5b3fd1",
              flex: "0 0 auto",
            }}
          >
            ACP
          </span>
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 10.5, color: "var(--ink-3)" }}>
            {session.model ?? t("agentZonePanel.modelPending")}
          </span>
        </div>
      </div>
      <span
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 5,
          height: 22,
          padding: "0 10px",
          borderRadius: "var(--r-pill)",
          fontSize: 11,
          fontWeight: 600,
          flex: "0 0 auto",
          background: tone.bg,
          color: tone.fg,
        }}
      >
        <span
          aria-hidden="true"
          style={{
            width: 8,
            height: 8,
            borderRadius: "50%",
            flex: "0 0 auto",
            background: tone.dot,
            animation: tone.pulse ? "yzpulse 1.4s ease-in-out infinite" : undefined,
          }}
        />
        {tone.label}
      </span>
    </div>
  )
}

function TranscriptList({
  sessionId,
  session,
  agent,
}: {
  sessionId: string
  session: SessionState
  agent: AgentVisual
}) {
  const { t } = useTranslation("panels")
  return (
    <div
      className="yzs"
      style={{
        flex: 1,
        minHeight: 0,
        overflow: "auto",
        padding: "16px 18px",
        display: "flex",
        flexDirection: "column",
        gap: 12,
        background: "var(--yz-sunk)",
      }}
    >
      {session.transcript.length === 0 ? (
        <div className="flex min-h-full items-center justify-center text-[12.5px] text-(--ink-3)">
          {t("agentZonePanel.noTranscript")}
        </div>
      ) : (
        session.transcript.map((entry, index) => (
          <TranscriptEntryRow
            key={`${index}-${entryKey(entry)}`}
            entry={entry}
            sessionId={sessionId}
            agent={agent}
          />
        ))
      )}
    </div>
  )
}

function TranscriptEntryRow({
  entry,
  sessionId,
  agent,
}: {
  entry: TranscriptEntry
  sessionId: string
  agent: AgentVisual
}) {
  const { t } = useTranslation("panels")
  if ("who" in entry) {
    const you = entry.who === "you"
    return (
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          alignItems: you ? "flex-end" : "flex-start",
        }}
      >
        <span
          style={{
            fontSize: 9.5,
            fontWeight: 600,
            letterSpacing: "0.04em",
            textTransform: "uppercase",
            color: "var(--ink-3)",
            padding: "0 4px",
          }}
        >
          {you ? t("agentZonePanel.you") : agent.label}
        </span>
        <div
          style={{
            maxWidth: "82%",
            padding: "10px 13px",
            borderRadius: 14,
            fontSize: 12.5,
            lineHeight: 1.55,
            whiteSpace: "pre-wrap",
            ...(you
              ? {
                  background: "var(--yz-active)",
                  color: "var(--ink-0)",
                  border: "1px solid var(--line-1)",
                  borderBottomRightRadius: 5,
                }
              : {
                  background: "var(--paper-1)",
                  color: "var(--ink-1)",
                  border: "1px solid var(--line-1)",
                  borderBottomLeftRadius: 5,
                }),
          }}
        >
          {you ? entry.text : <MinimalMarkdown text={entry.text} />}
          {entry.streaming && <StreamingCursor />}
        </div>
      </div>
    )
  }

  return <TranscriptBlock entry={entry} sessionId={sessionId} />
}

function TranscriptBlock({ entry, sessionId }: { entry: BlockEntry; sessionId: string }) {
  const respondPermission = useAgentStore((s) => s.respondPermission)
  const openText = useDiffModalStore((s) => s.openText)
  const style = THREAD_KIND_STYLE[entry.kind] ?? THREAD_KIND_STYLE.tool
  const isMono = entry.kind === "tool"

  function onAction(action: TranscriptAction) {
    if (entry.kind === "diff" && action.kind === "view_diff") {
      const diff = diffPayloadFromAction(action)
      if (diff) {
        openText(
          diff.path,
          { kind: "full", content: diff.oldText ?? "" },
          { kind: "full", content: diff.newText }
        )
      }
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
          background: style.bg,
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
        {entry.meta && (
          <span
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: 10.5,
              color: "var(--ink-3)",
              flex: "0 0 auto",
            }}
          >
            {displayMeta(entry)}
          </span>
        )}
        {entry.actions && entry.actions.length > 0 && (
          <div style={{ display: "flex", gap: 7, flex: "0 0 auto" }}>
            {entry.actions.map((action, index) => (
              <button
                key={`${action.label}-${index}`}
                type="button"
                onClick={() => onAction(action)}
                style={actionChipStyle(index === 0, style.accent)}
              >
                {action.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

function SlashPopup({
  commands,
  selectedIndex,
  onPick,
}: {
  commands: SlashCommand[]
  selectedIndex: number
  onPick: (command: SlashCommand) => void
}) {
  const { t } = useTranslation("panels")
  return (
    <div
      className="yzs"
      role="listbox"
      aria-label={t("agentZonePanel.slashCommands")}
      style={{
        position: "absolute",
        left: 14,
        right: 14,
        bottom: 60,
        zIndex: 20,
        maxHeight: 300,
        overflowY: "auto",
        background: "var(--frost-light)",
        backdropFilter: "var(--blur-frost)",
        WebkitBackdropFilter: "var(--blur-frost)",
        border: "1px solid var(--line-2)",
        borderRadius: 14,
        boxShadow: "var(--shadow-xl)",
        padding: 7,
        animation: "yzpop 130ms var(--ease-spring)",
      }}
    >
      <div
        style={{
          font: "var(--text-label)",
          fontSize: 9.5,
          letterSpacing: "0.09em",
          color: "var(--ink-3)",
          textTransform: "uppercase",
          padding: "8px 10px 5px",
        }}
      >
        {t("agentZonePanel.commandsHeader")}
      </div>
      {commands.length === 0 ? (
        <div className="px-[11px] py-[10px] text-[12px] text-(--ink-3)">{t("agentZonePanel.noCommands")}</div>
      ) : (
        commands.map((command, index) => {
          const selected = index === selectedIndex
          return (
            <button
              key={command.name}
              type="button"
              role="option"
              aria-label={`/${command.name} ${command.description}`}
              aria-selected={selected}
              onClick={() => onPick(command)}
              style={{
                width: "100%",
                display: "flex",
                alignItems: "center",
                gap: 11,
                height: 38,
                padding: "0 11px",
                borderRadius: 10,
                cursor: "pointer",
                transition: "background 110ms",
                background: selected ? "var(--yz-active)" : "transparent",
                boxShadow: selected ? "inset 0 0 0 1px var(--line-1)" : undefined,
                border: 0,
                textAlign: "left",
              }}
            >
              <span
                aria-hidden="true"
                style={{
                  width: 26,
                  height: 26,
                  flex: "0 0 auto",
                  borderRadius: 8,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  background: "var(--mint-soft)",
                  color: "#0f7a55",
                }}
              >
                <Slash className="size-[15px]" aria-hidden="true" />
              </span>
              <span
                style={{
                  fontFamily: "var(--font-mono)",
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: "var(--ink-0)",
                  flex: "0 0 auto",
                }}
              >
                /{command.name}
              </span>
              <span
                style={{
                  flex: 1,
                  minWidth: 0,
                  fontSize: 12,
                  color: "var(--ink-3)",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {command.description}
              </span>
            </button>
          )
        })
      )}
    </div>
  )
}

function IconButton({
  label,
  title,
  icon: Icon,
  onClick,
  disabled,
  style,
}: {
  label: string
  title: string
  icon: LucideIcon
  onClick: () => void
  disabled?: boolean
  style: CSSProperties
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: disabled ? "not-allowed" : "pointer",
        flex: "0 0 auto",
        border: "none",
        opacity: disabled ? 0.5 : 1,
        ...style,
      }}
    >
      <Icon className="size-[16px]" aria-hidden="true" />
    </button>
  )
}

function MinimalMarkdown({ text }: { text: string }) {
  return <>{renderMarkdownBlocks(text)}</>
}

function renderMarkdownBlocks(text: string): ReactNode[] {
  const nodes: ReactNode[] = []
  const fence = /```([^\n`]*)\n?([\s\S]*?)```/g
  let lastIndex = 0
  let match: RegExpExecArray | null

  while ((match = fence.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(
        <span key={`txt-${lastIndex}`} className="whitespace-pre-wrap">
          {renderInlineMarkdown(text.slice(lastIndex, match.index), `in-${lastIndex}`)}
        </span>
      )
    }
    const code = match[2] ?? ""
    nodes.push(
      <pre
        key={`pre-${match.index}`}
        style={{
          margin: "8px 0 0",
          padding: "8px 10px",
          borderRadius: 8,
          background: "var(--yz-field)",
          border: "1px solid var(--line-1)",
          overflowX: "auto",
          fontFamily: "var(--font-mono)",
          fontSize: 11.5,
          lineHeight: 1.5,
        }}
      >
        <code>{code.replace(/\n$/, "")}</code>
      </pre>
    )
    lastIndex = fence.lastIndex
  }

  if (lastIndex < text.length) {
    nodes.push(
      <span key={`txt-${lastIndex}`} className="whitespace-pre-wrap">
        {renderInlineMarkdown(text.slice(lastIndex), `in-${lastIndex}`)}
      </span>
    )
  }

  return nodes.length > 0 ? nodes : [text]
}

function renderInlineMarkdown(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = []
  let index = 0
  let key = 0

  while (index < text.length) {
    const codeIndex = text.indexOf("`", index)
    const boldIndex = text.indexOf("**", index)
    const next = nextMarker(codeIndex, boldIndex)

    if (next === -1) {
      nodes.push(text.slice(index))
      break
    }

    if (next > index) nodes.push(text.slice(index, next))

    if (next === codeIndex) {
      const end = text.indexOf("`", codeIndex + 1)
      if (end === -1) {
        nodes.push(text.slice(codeIndex))
        break
      }
      nodes.push(
        <code
          key={`${keyPrefix}-code-${key++}`}
          style={{
            padding: "1px 4px",
            borderRadius: 5,
            background: "var(--yz-field)",
            border: "1px solid var(--line-1)",
            fontFamily: "var(--font-mono)",
            fontSize: "0.92em",
          }}
        >
          {text.slice(codeIndex + 1, end)}
        </code>
      )
      index = end + 1
      continue
    }

    const end = text.indexOf("**", boldIndex + 2)
    if (end === -1) {
      nodes.push(text.slice(boldIndex))
      break
    }
    nodes.push(
      <strong key={`${keyPrefix}-strong-${key++}`}>{text.slice(boldIndex + 2, end)}</strong>
    )
    index = end + 2
  }

  return nodes
}

function nextMarker(codeIndex: number, boldIndex: number): number {
  if (codeIndex === -1) return boldIndex
  if (boldIndex === -1) return codeIndex
  return Math.min(codeIndex, boldIndex)
}

function StreamingCursor() {
  return (
    <span
      data-testid="agent-streaming-cursor"
      aria-hidden="true"
      style={{
        display: "inline-block",
        width: 7,
        height: 13,
        marginLeft: 3,
        verticalAlign: -2,
        background: "#5b3fd1",
        borderRadius: 2,
        animation: "yzblink 1.1s step-end infinite",
      }}
    />
  )
}

function actionChipStyle(primary: boolean, accent: string): CSSProperties {
  return {
    display: "inline-flex",
    alignItems: "center",
    height: 26,
    padding: "0 12px",
    borderRadius: 8,
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

function diffPayloadFromAction(action: TranscriptAction): AgentDiffPayload | null {
  const payload = action.payload
  if (!payload || typeof payload !== "object") return null
  const record = payload as Record<string, unknown>
  if (typeof record.path !== "string" || typeof record.newText !== "string") return null
  return {
    path: record.path,
    oldText: typeof record.oldText === "string" ? record.oldText : null,
    newText: record.newText,
  }
}

function displayMeta(entry: BlockEntry): string {
  if (!entry.meta) return ""
  try {
    const parsed = JSON.parse(entry.meta) as Record<string, unknown>
    if (entry.kind === "diff" && typeof parsed.path === "string") return parsed.path
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

function entryKey(entry: TranscriptEntry): string {
  return "who" in entry ? `${entry.who}:${entry.text}` : `${entry.kind}:${entry.text}`
}

function agentVisual(label: string, fallback: string): AgentVisual {
  const normalized = label.trim().toLowerCase()
  const known = Object.entries(AGENT_TYPES).find(([key]) => normalized.includes(key))?.[1]
  if (known) return known
  const display = label.trim() || fallback
  return {
    label: display,
    short: display.slice(0, 1).toUpperCase(),
    color: "#5b3fd1",
    bg: "rgba(91,63,209,0.13)",
  }
}

function hexAlpha(hex: string, alpha: number): string {
  const normalized = hex.replace("#", "")
  if (normalized.length !== 6) return hex
  const r = Number.parseInt(normalized.slice(0, 2), 16)
  const g = Number.parseInt(normalized.slice(2, 4), 16)
  const b = Number.parseInt(normalized.slice(4, 6), 16)
  return `rgba(${r},${g},${b},${alpha})`
}
