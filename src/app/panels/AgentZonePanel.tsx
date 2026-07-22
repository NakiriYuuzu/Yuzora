import {
  Bot,
  Check,
  ChevronDown,
  FileText,
  Paperclip,
  SendHorizontal,
  Slash,
  Sparkles,
  Square,
  X,
} from "lucide-react"
import { open as openImageFileDialog } from "@tauri-apps/plugin-dialog"
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
} from "react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { resolvePrewarmAgentId } from "@/app/workbench/settingsStorage"
import { readFileBase64 } from "@/lib/ipc"
import { Badge } from "@/components/ui/badge"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupText,
  InputGroupTextarea,
} from "@/components/ui/input-group"
import { TranscriptList, type AgentVisual } from "@/app/panels/agentzone/TranscriptList"
import { ChangesSummary } from "@/app/panels/agentzone/ChangesSummary"
import { ElicitationDialog } from "@/app/panels/agentzone/ElicitationDialog"
import { collectChangeStats } from "@/app/panels/agentzone/changeStats"
import {
  ComposerSuggestionPopup,
  type ComposerSuggestionItem,
} from "@/app/panels/ComposerSuggestionPopup"
import {
  INITIAL_COMPOSER_SUGGESTION_STATE,
  applyComposerSuggestion,
  buildSkillPromptText,
  clampSuggestionIndex,
  composerSuggestionOptionId,
  composerSuggestionReducer,
  filterAgentSkills,
  filterComposerSuggestions,
  insertSlashTrigger,
  parseComposerSuggestionTrigger,
  partitionAgentCommands,
  stripOrdinarySlashCommandPrefix,
  stripOrdinarySlashCommandPrefixWithCaret,
  type AgentSkillCommand,
} from "@/app/panels/agentComposerSuggestions"
import {
  canonicalPathKey,
  rankWorkspaceMentions,
  workspaceMentionIndex,
  type RankedWorkspaceMention,
  type WorkspaceMentionIndexSnapshot,
} from "@/agent/workspaceMentionIndex"
import type {
  PromptBlock,
  SessionConfigOption,
  SessionConfigSelectGroup,
  SessionConfigSelectOption,
  SlashCommand,
} from "@/agent/acpConnection"
import {
  AGENT_VISUALS,
  CUSTOM_AGENT_VISUAL,
  agentDisplayName,
  type AgentPreset,
} from "@/lib/agentPresets"
import {
  firstAbsolutePath,
  workspacePathBasename,
  workspacePathForDisplay,
} from "@/lib/paths"
import type { WorkspacePathIndexEntry } from "@/lib/types"
import { pathToUri } from "@/lsp/workspace"
import type { AgentTone, SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { normalizeWorkspacePath } from "@/state/recentWorkspaces"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

type ToneVisual = {
  label: string
  dot: string
  bg: string
  fg: string
  pulse?: boolean
}

// agentId→視覺對照的單一來源是 lib/agentPresets 的 AGENT_VISUALS；這裡只是把它
// 轉成本檔既有的 { label, short, color, bg } 介面，避免與 AgentNavContent 各自
// 硬編一份同樣的對照而漂移。
const AGENT_TYPES: Record<string, AgentVisual> = Object.fromEntries(
  Object.entries(AGENT_VISUALS).map(([id, visual]) => [
    id,
    { label: visual.label, short: visual.glyph, color: visual.colorVar, bg: visual.softVar },
  ])
)

// Tone label text is localized at the call site (ActiveAgentSession) via the
// component's `t`; this map only carries the tone's non-text styling.
const TONE_STYLE: Record<AgentTone, Omit<ToneVisual, "label">> = {
  idle: { dot: "var(--ink-4)", bg: "var(--paper-2)", fg: "var(--ink-3)" },
  run: {
    dot: "var(--yz-run)",
    bg: "var(--run-soft)",
    fg: "#b45309",
    pulse: true,
  },
  done: { dot: "#2bbf8a", bg: "var(--mint-soft)", fg: "#0f7a55" },
  wait: { dot: "#ffb23e", bg: "var(--amber-soft)", fg: "#9a6512" },
  fail: { dot: "#e23b54", bg: "var(--danger-soft)", fg: "#b51f38" },
}

function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path
}

type FileIndexViewState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; snapshot: WorkspaceMentionIndexSnapshot }
  | { status: "error"; message: string }

type ComposerSuggestionChoice =
  | { kind: "slash"; command: SlashCommand }
  | { kind: "skill"; skill: AgentSkillCommand }
  | { kind: "file"; file: RankedWorkspaceMention }

interface ComposerAttachment {
  canonicalPath: string
  label: string
  resourceName: string
}

// 圖片附件（貼上／上傳）：與 file mention 分開的 state，dataUrl 供 chip 縮圖
// 直接使用；送出時剝除 data: 前綴轉 ACP ImageContent 的純 base64（t4-2a）。
interface ComposerImageAttachment {
  id: string
  label: string
  mimeType: string
  byteSize: number
  dataUrl: string
}

// 限制值（plan Q4）：mime 白名單、單張原始 ≤5MB、每 turn ≤8 張。
const IMAGE_ATTACHMENT_MIME_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
])
const IMAGE_ATTACHMENT_MAX_BYTES = 5 * 1024 * 1024
const IMAGE_ATTACHMENT_MAX_COUNT = 8
const IMAGE_EXTENSION_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
}

function dataUrlToBase64(dataUrl: string): string {
  return dataUrl.slice(dataUrl.indexOf(",") + 1)
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
  const pendingNewSession = useAgentStore((s) => s.pendingNewSession)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const activatedWorkspaceKeyRef = useRef<string | null>(null)
  const connection = useAgentStore((s) => s.connection)
  const activateDraftWorkspace = useAgentStore((s) => s.activateDraftWorkspace)
  const ensureDraftSession = useAgentStore((s) => s.ensureDraftSession)
  const draftCwd = firstAbsolutePath(workspacePath)
  const draftWorkspaceHydrated = useAgentStore((s) => draftCwd
    ? s.hydratedWorkspaceCwds.has(normalizeWorkspacePath(draftCwd))
    : false)
  const showConnectionError = !authRequired
    && (connectionState === "error" || connectionError !== null)
  // cwd 防呆：沒有絕對路徑的 workspace／session cwd 時顯示引導，避免以相對路徑 spawn。
  const cwd = firstAbsolutePath(workspacePath, session?.cwd)
  const showWorkspaceGuide = !authRequired
    && !showConnectionError
    && cwd === null
  // P10-B：主面板只在 session 明確屬於「不同」workspace cwd 時才隱藏，
  // 避免切到 workspace B 時仍操作到 workspace A 的 session；
  // 其餘情況（cwd 未知等）維持顯示，避免誤判成別的 workspace。
  // 目前 cwd 為絕對路徑、但 session 沒有相符 cwd（含 null）時，也視為別的
  // workspace 而隱藏——否則面板仍顯示該 session，但 sendPrompt 卻不會沿用它。
  const belongsToOtherWorkspace = Boolean(
    session && cwd
    && (!session.cwd || normalizeWorkspacePath(session.cwd) !== normalizeWorkspacePath(cwd))
  )
  const scopedSession = belongsToOtherWorkspace ? null : session

  // Activate the process-wide mention index at workspace scope, not session
  // scope: changing sessions within one workspace must keep query reuse, while
  // A→B→A (or remounting this panel) must invalidate A's previous watcher
  // lifetime. Layout timing guarantees this happens before child load effects.
  useLayoutEffect(() => {
    const workspaceKey = workspacePath ? canonicalPathKey(workspacePath) : null
    if (activatedWorkspaceKeyRef.current === workspaceKey) return
    activatedWorkspaceKeyRef.current = workspaceKey
    if (workspacePath) workspaceMentionIndex.activateWorkspace(workspacePath)
  }, [workspacePath])

  // Workspace generation changes immediately with the visible absolute cwd,
  // before hydration/auto-create can settle. This keeps a late draft from the
  // previous workspace from becoming active in the new one. Deliberately no
  // unmount cleanup: leaving Agent mode must preserve an already-created draft.
  useEffect(() => {
    if (draftCwd) activateDraftWorkspace(draftCwd)
  }, [activateDraftWorkspace, draftCwd])

  useEffect(() => {
    if (!draftCwd || !draftWorkspaceHydrated || !connection) return
    if (authRequired || showConnectionError || pendingNewSession) return
    const agentId = resolvePrewarmAgentId()
    if (!agentId) return
    void ensureDraftSession(draftCwd, agentId).catch(() => undefined)
  }, [
    authRequired,
    connection,
    draftCwd,
    draftWorkspaceHydrated,
    ensureDraftSession,
    pendingNewSession,
    showConnectionError,
  ])

  return (
    <div
      className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)"
    >
      {authRequired && <AuthRequiredBanner />}
      {showConnectionError && <ConnectionErrorBanner />}
      {showWorkspaceGuide && <WorkspaceGuideBanner />}
      {pendingNewSession ? (
        <ConnectingState />
      ) : scopedSession && activeSessionId ? (
        <ActiveAgentSession key={activeSessionId} sessionId={activeSessionId} session={scopedSession} />
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

function ConnectingState() {
  const { t } = useTranslation("panels")
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center">
      <EmptyState
        icon={Bot}
        title={t("agentZonePanel.connecting")}
        description={t("agentZonePanel.connectingHint")}
      />
    </div>
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
  const [suggestions, dispatchSuggestion] = useReducer(
    composerSuggestionReducer,
    INITIAL_COMPOSER_SUGGESTION_STATE
  )
  const [fileIndexState, setFileIndexState] = useState<FileIndexViewState>({ status: "idle" })
  const [explicitAttachments, setExplicitAttachments] = useState<WorkspacePathIndexEntry[]>([])
  const [imageAttachments, setImageAttachments] = useState<ComposerImageAttachment[]>([])
  // 同步權威：paste 的多個 FileReader.onload 與檔案多選的 await 迴圈都在「舊
  // render 的 closure」內連續呼叫 addImageAttachments，直接讀 state 會拿到過期
  // 值而超收（張數上限失效）。所有寫入一律走 writeImageAttachments，ref 先行、
  // state 跟隨。
  const imageAttachmentsRef = useRef<ComposerImageAttachment[]>([])
  const [composerNotice, setComposerNotice] = useState<string | null>(null)
  const composerNoticeTimerRef = useRef<number | null>(null)
  const [selectedSkillRawName, setSelectedSkillRawName] = useState<string | null>(null)
  const [removedFilePath, setRemovedFilePath] = useState<string | null>(null)
  const connection = useAgentStore((s) => s.connection)
  const sendPrompt = useAgentStore((s) => s.sendPrompt)
  const cancel = useAgentStore((s) => s.cancel)
  const composerFocusRequest = useAgentStore((s) => s.composerFocusRequest)
  const composerRef = useRef<HTMLTextAreaElement>(null)
  const isComposingRef = useRef(false)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const treeRevision = useWorkspaceStore((s) => s.treeRevision)
  const activeFilePath = useWorkspaceStore((s) => {
    const path = s.groups[s.activeGroupIndex]?.activePath ?? null
    return path === PREVIEW_TAB_PATH ? null : path
  })
  const composerWorkspaceKeyRef = useRef(workspacePath ? canonicalPathKey(workspacePath) : null)

  const agent = agentVisual(session.agentId, session.agentLabel, t("agentZonePanel.agentFallback"))
  const tone: ToneVisual = { ...TONE_STYLE[session.tone], label: t(`agentZonePanel.tone.${session.tone}`) }
  const commandPartition = useMemo(
    () => partitionAgentCommands(session.agentId, session.availableCommands),
    [session.agentId, session.availableCommands]
  )
  const changeStats = useMemo(() => collectChangeStats(session.transcript), [session.transcript])
  const activeSkill = selectedSkillRawName
    ? commandPartition.skills.find((skill) => skill.rawName === selectedSkillRawName) ?? null
    : null
  const suggestionTrigger = suggestions.trigger
  const suggestionsOpen = Boolean(suggestionTrigger && !suggestions.dismissed)
  const slashSuggestionsOpen = suggestionsOpen && suggestionTrigger?.kind === "slash"
  const fileSuggestionsOpen = suggestionsOpen && suggestionTrigger?.kind === "file"
  const fileSnapshot = fileIndexState.status === "ready"
    && workspacePath
    && fileIndexState.snapshot.revision === treeRevision
    && canonicalPathKey(fileIndexState.snapshot.workspace) === canonicalPathKey(workspacePath)
    ? fileIndexState.snapshot
    : null
  const slashCommands = suggestionTrigger?.kind === "slash" && suggestionsOpen
    ? filterComposerSuggestions(
        commandPartition.slashCommands,
        suggestionTrigger.query,
        (command) => command.name
      )
    : []
  const skillCommands = suggestionTrigger?.kind === "skill" && suggestionsOpen
    ? filterAgentSkills(commandPartition.skills, suggestionTrigger.query)
    : []
  const fileCommands = suggestionTrigger?.kind === "file" && suggestionsOpen && fileSnapshot
    ? rankWorkspaceMentions(fileSnapshot.entries, suggestionTrigger.query)
    : []
  const suggestionListboxId = `agent-composer-suggestions-${encodeURIComponent(sessionId)}`
  const suggestionItems: ComposerSuggestionItem<ComposerSuggestionChoice>[] = suggestionTrigger?.kind === "slash"
    ? slashCommands.map((command) => ({
        key: `slash:${command.name}`,
        value: { kind: "slash", command },
        ariaLabel: `/${command.name} ${command.description}`,
        label: `/${command.name}`,
        description: command.description,
        leading: <Slash className="size-[15px]" aria-hidden="true" />,
      }))
    : suggestionTrigger?.kind === "skill"
      ? skillCommands.map((skill) => ({
          key: `skill:${skill.rawName}`,
          value: { kind: "skill", skill },
          ariaLabel: t("agentZonePanel.skillSuggestion", {
            name: skill.rawName,
            description: skill.description,
          }),
          label: skill.rawName,
          description: skill.description,
          leading: <Sparkles className="size-[15px]" aria-hidden="true" />,
        }))
      : fileCommands.map((file) => ({
          key: `file:${file.canonicalPath}`,
          value: { kind: "file", file },
          ariaLabel: t("agentZonePanel.fileSuggestion", { path: file.relativePath }),
          label: file.relativePath,
          leading: <FileText className="size-[15px]" aria-hidden="true" />,
        }))
  const selectedSuggestionIndex = clampSuggestionIndex(
    suggestions.selectedIndex,
    suggestionItems.length
  )
  const activeSuggestionItem = suggestionsOpen
    ? suggestionItems[selectedSuggestionIndex]
    : undefined
  const activeSuggestionId = activeSuggestionItem
    ? composerSuggestionOptionId(suggestionListboxId, activeSuggestionItem.key)
    : undefined
  const turnInProgress = session.tone === "run" || session.pendingTurn || session.running === true
  const attachments = useMemo(() => {
    const merged = new Map<string, ComposerAttachment>()
    if (
      activeFilePath
      && (!removedFilePath || canonicalPathKey(activeFilePath) !== canonicalPathKey(removedFilePath))
    ) {
      merged.set(canonicalPathKey(activeFilePath), {
        canonicalPath: activeFilePath,
        label: fileNameFromPath(activeFilePath),
        resourceName: fileNameFromPath(activeFilePath),
      })
    }
    for (const entry of explicitAttachments) {
      const key = canonicalPathKey(entry.canonicalPath)
      if (!merged.has(key)) {
        merged.set(key, {
          canonicalPath: entry.canonicalPath,
          label: entry.relativePath,
          resourceName: entry.relativePath,
        })
      }
    }
    return [...merged.values()]
  }, [activeFilePath, explicitAttachments, removedFilePath])

  const popupStatus: "ready" | "loading" | "error" = suggestionTrigger?.kind === "file"
    ? fileIndexState.status === "error"
      ? "error"
      : fileSnapshot
        ? "ready"
        : "loading"
    : "ready"
  const popupAriaLabel = suggestionTrigger?.kind === "file"
    ? t("agentZonePanel.fileSuggestions")
    : suggestionTrigger?.kind === "skill"
      ? t("agentZonePanel.skillSuggestions")
      : t("agentZonePanel.slashCommands")
  const popupHeader = suggestionTrigger?.kind === "file"
    ? (
        <span>
          {t("agentZonePanel.filesHeader")}
          {fileSnapshot?.truncated && (
            <span role="status"> · {t("agentZonePanel.workspaceIndexTruncated")}</span>
          )}
        </span>
      )
    : suggestionTrigger?.kind === "skill"
      ? t("agentZonePanel.skillsHeader")
      : t("agentZonePanel.commandsHeader")
  const popupEmpty = suggestionTrigger?.kind === "file"
    ? t("agentZonePanel.noMatchingFiles")
    : suggestionTrigger?.kind === "skill"
      ? commandPartition.skillsSupported
        ? t("agentZonePanel.noSkills")
        : t("agentZonePanel.skillsUnsupported")
      : t("agentZonePanel.noCommands")

  useEffect(() => {
    if (
      activeFilePath
      && removedFilePath
      && canonicalPathKey(activeFilePath) !== canonicalPathKey(removedFilePath)
    ) {
      setRemovedFilePath(null)
    }
  }, [activeFilePath, removedFilePath])

  useEffect(() => {
    const workspaceKey = workspacePath ? canonicalPathKey(workspacePath) : null
    if (composerWorkspaceKeyRef.current === workspaceKey) return
    composerWorkspaceKeyRef.current = workspaceKey
    setComposer("")
    setExplicitAttachments([])
    setSelectedSkillRawName(null)
    setRemovedFilePath(null)
    setFileIndexState({ status: "idle" })
    dispatchSuggestion({ type: "reset" })
  }, [workspacePath])

  useEffect(() => {
    if (!fileSuggestionsOpen || !workspacePath) return
    let current = true
    queueMicrotask(() => {
      if (!current) return
      setFileIndexState((state) => state.status === "ready"
        && state.snapshot.revision === treeRevision
        && canonicalPathKey(state.snapshot.workspace) === canonicalPathKey(workspacePath)
        ? state
        : { status: "loading" })
    })
    void workspaceMentionIndex.load(workspacePath, treeRevision)
      .then((snapshot) => {
        if (current && snapshot) setFileIndexState({ status: "ready", snapshot })
      })
      .catch((error: unknown) => {
        if (!current) return
        setFileIndexState({
          status: "error",
          message: error instanceof Error ? error.message : String(error),
        })
      })
    return () => {
      current = false
    }
  }, [fileSuggestionsOpen, treeRevision, workspacePath])

  useEffect(() => {
    if (!selectedSkillRawName || activeSkill) return
    let current = true
    queueMicrotask(() => {
      if (current) setSelectedSkillRawName(null)
    })
    return () => {
      current = false
    }
  }, [activeSkill, selectedSkillRawName])

  useEffect(() => {
    if (composerFocusRequest?.sessionId === sessionId) {
      composerRef.current?.focus()
    }
  }, [composerFocusRequest, sessionId])

  useEffect(() => {
    if (suggestions.selectedIndex !== selectedSuggestionIndex) {
      dispatchSuggestion({ type: "options-changed", optionCount: suggestionItems.length })
    }
  }, [selectedSuggestionIndex, suggestionItems.length, suggestions.selectedIndex])

  function optionCountForTrigger(trigger: NonNullable<typeof suggestionTrigger>): number {
    if (trigger.kind === "slash") {
      return filterComposerSuggestions(
        commandPartition.slashCommands,
        trigger.query,
        (command) => command.name
      ).length
    }
    if (trigger.kind === "skill") {
      return filterAgentSkills(commandPartition.skills, trigger.query).length
    }
    return fileSnapshot ? rankWorkspaceMentions(fileSnapshot.entries, trigger.query).length : 0
  }

  function queueComposerFocus(caret?: number) {
    queueMicrotask(() => {
      const textarea = composerRef.current
      if (!textarea) return
      textarea.focus()
      if (caret !== undefined) textarea.setSelectionRange(caret, caret)
    })
  }

  function openSlash() {
    if (isComposingRef.current) return
    const caret = composerRef.current?.selectionStart ?? composer.length
    if (slashSuggestionsOpen) {
      dispatchSuggestion({ type: "dismiss" })
      queueComposerFocus(caret)
      return
    }
    const existingTrigger = parseComposerSuggestionTrigger(composer, caret)
    if (existingTrigger?.kind === "slash") {
      dispatchSuggestion({ type: "open-trigger", trigger: existingTrigger })
      queueComposerFocus(caret)
      return
    }
    const inserted = insertSlashTrigger(composer, caret)
    setComposer(inserted.text)
    dispatchSuggestion({ type: "open-trigger", trigger: inserted.trigger })
    queueComposerFocus(inserted.caret)
  }

  function updateComposer(value: string, caret: number, isComposing: boolean) {
    setComposer(value)
    if (
      selectedSkillRawName
      && stripOrdinarySlashCommandPrefix(value, commandPartition.slashCommands) !== value.trim()
    ) {
      setSelectedSkillRawName(null)
    }
    if (isComposing) {
      dispatchSuggestion({ type: "reset" })
      return
    }
    const trigger = parseComposerSuggestionTrigger(value, caret)
    dispatchSuggestion({
      type: "sync-trigger",
      trigger,
      optionCount: trigger ? optionCountForTrigger(trigger) : 0,
    })
  }

  function pickSlash(command: SlashCommand) {
    if (isComposingRef.current || suggestionTrigger?.kind !== "slash") return
    const applied = applyComposerSuggestion(composer, suggestionTrigger, `/${command.name} `)
    setComposer(applied.text)
    setSelectedSkillRawName(null)
    dispatchSuggestion({ type: "reset" })
    queueComposerFocus(applied.caret)
  }

  function pickSkill(skill: AgentSkillCommand) {
    if (isComposingRef.current || suggestionTrigger?.kind !== "skill") return
    const applied = applyComposerSuggestion(composer, suggestionTrigger, "")
    const next = stripOrdinarySlashCommandPrefixWithCaret(
      applied.text,
      applied.caret,
      commandPartition.slashCommands
    )
    setComposer(next.text)
    setSelectedSkillRawName(skill.rawName)
    dispatchSuggestion({ type: "reset" })
    queueComposerFocus(next.caret)
  }

  function pickFile(file: WorkspacePathIndexEntry) {
    if (isComposingRef.current || suggestionTrigger?.kind !== "file") return
    const applied = applyComposerSuggestion(composer, suggestionTrigger, "")
    setComposer(applied.text)
    setExplicitAttachments((entries) => {
      const key = canonicalPathKey(file.canonicalPath)
      return entries.some((entry) => canonicalPathKey(entry.canonicalPath) === key)
        ? entries
        : [...entries, { relativePath: file.relativePath, canonicalPath: file.canonicalPath }]
    })
    dispatchSuggestion({ type: "reset" })
    queueComposerFocus(applied.caret)
  }

  function pickSuggestion(choice: ComposerSuggestionChoice) {
    if (choice.kind === "slash") pickSlash(choice.command)
    else if (choice.kind === "skill") pickSkill(choice.skill)
    else pickFile(choice.file)
  }

  function writeImageAttachments(next: ComposerImageAttachment[]) {
    imageAttachmentsRef.current = next
    setImageAttachments(next)
  }

  function clearComposerIntent() {
    setComposer("")
    setExplicitAttachments([])
    writeImageAttachments([])
    setSelectedSkillRawName(null)
    setRemovedFilePath(null)
    dispatchSuggestion({ type: "reset" })
  }

  // 依 promptCapabilities.image 做 feature detection（C3）：能力未知（連線未建立、
  // restored 未 respawn）一律視同不支援——隱藏入口而非猜測。
  const supportsImageAttachments = Boolean(connection?.supportsImagePrompt?.(sessionId))

  function showComposerNotice(text: string) {
    setComposerNotice(text)
    if (composerNoticeTimerRef.current !== null) {
      window.clearTimeout(composerNoticeTimerRef.current)
    }
    composerNoticeTimerRef.current = window.setTimeout(() => {
      setComposerNotice(null)
      composerNoticeTimerRef.current = null
    }, 4000)
  }

  useEffect(() => () => {
    if (composerNoticeTimerRef.current !== null) {
      window.clearTimeout(composerNoticeTimerRef.current)
    }
  }, [])

  // 貼上與上傳共用的驗證管線：mime 白名單 → 空檔 → 單張大小 → 張數上限（Q4）。
  // 任何拒絕以 composer notice 告知原因。容量判斷讀 imageAttachmentsRef（見宣告
  // 處說明），通知與寫入都留在 handler 層，state updater 保持純函式。
  function addImageAttachments(
    candidates: { label: string; mimeType: string; byteSize: number; dataUrl: string }[]
  ): void {
    if (!supportsImageAttachments) {
      showComposerNotice(t("agentZonePanel.imageNotSupported", { agent: agent.label }))
      return
    }
    const accepted: ComposerImageAttachment[] = []
    for (const candidate of candidates) {
      if (!IMAGE_ATTACHMENT_MIME_TYPES.has(candidate.mimeType)) {
        showComposerNotice(t("agentZonePanel.imageBadType"))
        continue
      }
      // 0-byte 圖（空檔／讀出空內容）拒收，避免送出空 base64 的 image block。
      if (candidate.byteSize <= 0) {
        showComposerNotice(t("agentZonePanel.imageEmpty"))
        continue
      }
      if (candidate.byteSize > IMAGE_ATTACHMENT_MAX_BYTES) {
        showComposerNotice(t("agentZonePanel.imageTooLarge", { max: "5MB" }))
        continue
      }
      accepted.push({
        // eslint-disable-next-line react-hooks/purity -- called only from paste/file-picker callbacks
        id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        label: candidate.label,
        mimeType: candidate.mimeType,
        byteSize: candidate.byteSize,
        dataUrl: candidate.dataUrl,
      })
    }
    if (accepted.length === 0) return
    const current = imageAttachmentsRef.current
    const room = IMAGE_ATTACHMENT_MAX_COUNT - current.length
    if (room <= 0) {
      showComposerNotice(t("agentZonePanel.imageTooMany", { max: IMAGE_ATTACHMENT_MAX_COUNT }))
      return
    }
    if (accepted.length > room) {
      showComposerNotice(t("agentZonePanel.imageTooMany", { max: IMAGE_ATTACHMENT_MAX_COUNT }))
    }
    writeImageAttachments([...current, ...accepted.slice(0, room)])
  }

  function onComposerPaste(event: React.ClipboardEvent<HTMLTextAreaElement>) {
    const items = [...(event.clipboardData?.items ?? [])]
    const imageItems = items.filter((item) => item.type.startsWith("image/"))
    if (imageItems.length === 0) return
    // 圖片部分自行處理；文字部分交還預設貼上行為（不 preventDefault 文字）。
    if (!supportsImageAttachments) {
      showComposerNotice(t("agentZonePanel.imageNotSupported", { agent: agent.label }))
      return
    }
    for (const item of imageItems) {
      const file = item.getAsFile()
      if (!file) continue
      const mimeType = file.type
      const reader = new FileReader()
      reader.onload = () => {
        if (typeof reader.result !== "string") return
        addImageAttachments([{
          label: file.name || t("agentZonePanel.pastedImageLabel"),
          mimeType,
          byteSize: file.size,
          dataUrl: reader.result,
        }])
      }
      reader.onerror = () => {
        showComposerNotice(t("agentZonePanel.imageReadFailed"))
      }
      reader.readAsDataURL(file)
    }
  }

  async function pickAndAttachImages() {
    const selected = await openImageFileDialog({
      multiple: true,
      filters: [{ name: "Images", extensions: Object.keys(IMAGE_EXTENSION_MIME) }],
    }).catch(() => null)
    const paths = Array.isArray(selected) ? selected : typeof selected === "string" ? [selected] : []
    for (const path of paths) {
      const ext = path.includes(".") ? path.split(".").pop()!.toLowerCase() : ""
      const mimeType = IMAGE_EXTENSION_MIME[ext]
      if (!mimeType) {
        showComposerNotice(t("agentZonePanel.imageBadType"))
        continue
      }
      try {
        const file = await readFileBase64(path, IMAGE_ATTACHMENT_MAX_BYTES)
        addImageAttachments([{
          label: workspacePathBasename(path),
          mimeType,
          byteSize: file.size,
          dataUrl: `data:${mimeType};base64,${file.data}`,
        }])
      } catch {
        // 結構化錯誤（超限／不可讀）以大小提示概括——超限是唯一可預期案例。
        showComposerNotice(t("agentZonePanel.imageTooLarge", { max: "5MB" }))
      }
    }
  }

  function removeImageAttachment(id: string) {
    writeImageAttachments(imageAttachmentsRef.current.filter((entry) => entry.id !== id))
  }

  function removeAttachment(attachment: ComposerAttachment) {
    const key = canonicalPathKey(attachment.canonicalPath)
    setExplicitAttachments((entries) => entries.filter(
      (entry) => canonicalPathKey(entry.canonicalPath) !== key
    ))
    if (activeFilePath && canonicalPathKey(activeFilePath) === key) {
      setRemovedFilePath(activeFilePath)
    }
  }

  // soak 回饋 #4：turn 進行中也可送出（builtin adapter 走 pi 原生 steering、
  // 社群 adapter 排入其佇列）——不再以 turnInProgress 擋。
  function submitPrompt() {
    const prompt = activeSkill
      ? buildSkillPromptText(activeSkill.rawName, composer, commandPartition.slashCommands)
      : composer.trim()
    // 純圖片（無文字、無 skill）也可送出；三者皆空才擋。
    if (!prompt && imageAttachments.length === 0) return
    // cwd 防呆：沒有絕對路徑就不 spawn（引導訊息由 WorkspaceGuideBanner 顯示）。
    const cwd = firstAbsolutePath(workspacePath, session.cwd)
    if (!cwd) return
    const promptPayload: PromptBlock[] = [
      ...(prompt ? [{ type: "text", text: prompt } as PromptBlock] : []),
      ...attachments.map((attachment): PromptBlock => ({
        type: "resource_link",
        uri: pathToUri(attachment.canonicalPath),
        name: attachment.resourceName,
      })),
      ...imageAttachments.map((image): PromptBlock => ({
        type: "image",
        data: dataUrlToBase64(image.dataUrl),
        mimeType: image.mimeType,
      })),
    ]
    clearComposerIntent()
    void sendPrompt(cwd, promptPayload).catch(() => undefined)
  }

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (isComposingRef.current || event.nativeEvent.isComposing) return

    if (suggestionsOpen) {
      if (event.key === "ArrowDown") {
        event.preventDefault()
        dispatchSuggestion({ type: "move", delta: 1, optionCount: suggestionItems.length })
        return
      }
      if (event.key === "ArrowUp") {
        event.preventDefault()
        dispatchSuggestion({ type: "move", delta: -1, optionCount: suggestionItems.length })
        return
      }
      if (event.key === "Escape") {
        event.preventDefault()
        dispatchSuggestion({ type: "dismiss" })
        return
      }
      if ((event.key === "Enter" || event.key === "Tab") && suggestionItems.length > 0) {
        event.preventDefault()
        const selected = suggestionItems[selectedSuggestionIndex] ?? suggestionItems[0]
        if (selected) pickSuggestion(selected.value)
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
      {/* Atelier（P2）：transcript 與 composer 共用一層 ambient 漸層畫布，
          composer 以 frost 玻璃艙浮於其上。 */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          flex: 1,
          minHeight: 0,
          background: "var(--yz-bg)",
        }}
      >
      <TranscriptList sessionId={sessionId} session={session} agent={agent} />
      <ElicitationDialog sessionId={sessionId} />
      <div
        style={{
          position: "relative",
        }}
      >
        <div style={{ padding: "8px 12px 10px" }}>
          {changeStats && (
            <div style={{ marginBottom: 6 }}>
              <ChangesSummary stats={changeStats} />
            </div>
          )}
          <div style={{ position: "relative" }}>
            {suggestionsOpen && (
              <ComposerSuggestionPopup
                id={suggestionListboxId}
                ariaLabel={popupAriaLabel}
                items={suggestionItems}
                selectedIndex={selectedSuggestionIndex}
                onSelect={pickSuggestion}
                onAfterSelect={queueComposerFocus}
                header={popupHeader}
                status={popupStatus}
                loadingSlot={<span role="status">{t("agentZonePanel.workspaceIndexLoading")}</span>}
                emptySlot={popupEmpty}
                errorSlot={(
                  <span role="alert">
                    {t("agentZonePanel.workspaceIndexError", {
                      message: fileIndexState.status === "error" ? fileIndexState.message : "",
                    })}
                  </span>
                )}
                style={{ left: 0, right: 0, bottom: "calc(100% + 8px)" }}
              />
            )}
            <InputGroup
              data-testid="agent-composer"
              data-layout="stacked-toolbar"
              className="h-auto rounded-2xl border-(--line-1) bg-(--yz-glass) shadow-sm backdrop-blur-xl has-disabled:bg-(--yz-glass) has-disabled:opacity-100 dark:has-disabled:bg-(--yz-glass)"
            >
              {(activeSkill || attachments.length > 0 || imageAttachments.length > 0) && (
                <InputGroupAddon
                  align="block-start"
                  role="list"
                  aria-label={t("agentZonePanel.composerIntent")}
                  className="flex-wrap gap-1.5 border-b border-(--line-1) px-2 py-1.5"
                >
                {activeSkill && (
                  <div
                    role="listitem"
                    aria-label={t("agentZonePanel.selectedSkill", { name: activeSkill.rawName })}
                    className="inline-flex h-6 items-center gap-1.5 rounded-full border border-[rgba(43,191,138,0.28)] bg-(--mint-soft) pl-2.5 pr-1 text-[11px] font-semibold text-[#0f7a55] shadow-xs"
                  >
                    <Sparkles className="size-3" aria-hidden="true" />
                    <span>{activeSkill.rawName}</span>
                    <InputGroupButton
                      aria-label={t("agentZonePanel.removeSkill", { name: activeSkill.rawName })}
                      size="icon-xs"
                      variant="ghost"
                      onClick={() => setSelectedSkillRawName(null)}
                      className="size-4 rounded-full bg-[rgba(15,122,85,0.1)] text-[#0f7a55] hover:bg-[rgba(15,122,85,0.16)]"
                    >
                      <X aria-hidden="true" />
                    </InputGroupButton>
                  </div>
                )}
                {attachments.map((attachment) => (
                  <div
                    key={canonicalPathKey(attachment.canonicalPath)}
                    role="listitem"
                    title={workspacePathForDisplay(attachment.canonicalPath)}
                    className="inline-flex h-6 max-w-full min-w-0 items-center gap-1.5 rounded-full border border-(--line-2) bg-(--paper-0) pl-2.5 pr-1 text-(--ink-2) shadow-xs"
                  >
                    <span className="min-w-0 truncate text-[11px] font-semibold">
                      {attachment.label}
                    </span>
                    <InputGroupButton
                      aria-label={t("agentZonePanel.removeFileContext", { fileName: attachment.label })}
                      size="icon-xs"
                      variant="ghost"
                      onClick={() => removeAttachment(attachment)}
                      className="size-4 rounded-full bg-[rgba(27,26,23,0.06)] text-(--ink-3) hover:bg-[rgba(27,26,23,0.1)]"
                    >
                      <X aria-hidden="true" />
                    </InputGroupButton>
                  </div>
                ))}
                {imageAttachments.map((image) => (
                  <div
                    key={image.id}
                    role="listitem"
                    data-testid="composer-image-chip"
                    title={image.label}
                    className="inline-flex h-6 max-w-full min-w-0 items-center gap-1.5 rounded-full border border-(--line-2) bg-(--paper-0) pl-1 pr-1 text-(--ink-2) shadow-xs"
                  >
                    <img
                      src={image.dataUrl}
                      alt={image.label}
                      className="size-4 shrink-0 rounded-full object-cover"
                    />
                    <span className="min-w-0 truncate text-[11px] font-semibold">
                      {image.label}
                    </span>
                    <InputGroupButton
                      aria-label={t("agentZonePanel.removeImageAttachment", { name: image.label })}
                      size="icon-xs"
                      variant="ghost"
                      onClick={() => removeImageAttachment(image.id)}
                      className="size-4 rounded-full bg-[rgba(27,26,23,0.06)] text-(--ink-3) hover:bg-[rgba(27,26,23,0.1)]"
                    >
                      <X aria-hidden="true" />
                    </InputGroupButton>
                  </div>
                ))}
                </InputGroupAddon>
              )}
            <InputGroupTextarea
              ref={composerRef}
              rows={1}
              value={composer}
              role="combobox"
              aria-label={t("agentZonePanel.composerAriaLabel")}
              aria-expanded={suggestionsOpen}
              aria-controls={suggestionsOpen ? suggestionListboxId : undefined}
              aria-activedescendant={activeSuggestionId}
              aria-autocomplete="list"
              onChange={(event) => updateComposer(
                event.currentTarget.value,
                event.currentTarget.selectionStart ?? event.currentTarget.value.length,
                isComposingRef.current || Boolean((event.nativeEvent as InputEvent).isComposing)
              )}
              onSelect={(event) => {
                if (isComposingRef.current) return
                const trigger = parseComposerSuggestionTrigger(
                  event.currentTarget.value,
                  event.currentTarget.selectionStart ?? event.currentTarget.value.length
                )
                dispatchSuggestion({
                  type: "sync-trigger",
                  trigger,
                  optionCount: trigger ? optionCountForTrigger(trigger) : 0,
                })
              }}
              onCompositionStart={() => {
                isComposingRef.current = true
                dispatchSuggestion({ type: "reset" })
              }}
              onCompositionEnd={() => {
                isComposingRef.current = false
              }}
              onKeyDown={onComposerKeyDown}
              onPaste={onComposerPaste}
              placeholder={t("agentZonePanel.replyPlaceholder", { agent: agent.label })}
              className="max-h-32 min-h-11 px-3 pt-3 pb-2 text-[13px] leading-5 text-(--ink-1) placeholder:text-(--ink-4)"
            />
            <InputGroupAddon
              align="block-end"
              className="justify-between gap-2 border-t border-(--line-1) px-2 py-1.5"
              onClick={(event) => {
                if ((event.target as HTMLElement).closest("button")) return
                composerRef.current?.focus()
              }}
            >
              <div className="flex min-w-0 items-center gap-1">
                <InputGroupButton
                  aria-label={t("agentZonePanel.slashCommands")}
                  title={t("agentZonePanel.slashCommandsTitle")}
                  size="icon-xs"
                  variant="ghost"
                  onClick={openSlash}
                  className={slashSuggestionsOpen ? "bg-accent text-[#5b3fd1]" : "text-(--ink-2)"}
                >
                  <Slash aria-hidden="true" />
                </InputGroupButton>
                {supportsImageAttachments && (
                  <InputGroupButton
                    aria-label={t("agentZonePanel.attachImage")}
                    title={t("agentZonePanel.attachImageTitle")}
                    size="icon-xs"
                    variant="ghost"
                    onClick={() => void pickAndAttachImages()}
                    className="text-(--ink-2)"
                  >
                    <Paperclip aria-hidden="true" />
                  </InputGroupButton>
                )}
                {/* soak 回饋 #1/#5：執行中允許切換 model/effort（pi 本身隨時可切，
                    下一次 LLM 呼叫生效）；僅在 setter 在途時鎖住避免並發。 */}
                <SessionConfigControls
                  sessionId={sessionId}
                  session={session}
                  disabled={Boolean(session.configRequest)}
                />
              </div>

              <div className="ml-auto flex shrink-0 items-center gap-1">
                <InputGroupText
                  aria-hidden="true"
                  className="font-mono text-[9.5px] text-(--ink-4)"
                >
                  ⌘↵
                </InputGroupText>
                {turnInProgress && (
                  <InputGroupButton
                    aria-label={t("agentZonePanel.cancelAriaLabel")}
                    title={t("agentZonePanel.cancel")}
                    size="icon-sm"
                    variant="destructive"
                    className="rounded-full"
                    onClick={() => {
                      clearComposerIntent()
                      // cancel() records connectionError before rejecting; the panel's
                      // existing error surface renders it for toolbar-triggered failures.
                      void cancel(sessionId).catch(() => undefined)
                    }}
                  >
                    <Square aria-hidden="true" />
                  </InputGroupButton>
                )}
                {/* 執行中送出＝steering（下一句插進現行 turn）；與停止鈕並列。 */}
                <InputGroupButton
                  aria-label={t("agentZonePanel.sendAriaLabel")}
                  title={turnInProgress ? t("agentZonePanel.steerSend") : t("agentZonePanel.send")}
                  size="icon-sm"
                  variant="default"
                  onClick={submitPrompt}
                  disabled={!composer.trim() && !activeSkill && imageAttachments.length === 0}
                  className="rounded-full bg-(--yz-accent) text-[#223005] hover:bg-(--yz-accent-ink) hover:text-(--paper-0)"
                >
                  <SendHorizontal aria-hidden="true" />
                </InputGroupButton>
              </div>
            </InputGroupAddon>
            </InputGroup>
          </div>
          {composerNotice && (
            <div
              role="status"
              aria-live="polite"
              data-testid="composer-notice"
              className="px-1 pt-1.5 text-[10.5px] text-(--ink-3)"
              style={{ overflowWrap: "anywhere" }}
            >
              {composerNotice}
            </div>
          )}
          {session.configError && (
            <div
              role="alert"
              aria-label={t("agentZonePanel.configErrorAria")}
              className="px-1 pt-1.5 text-[10.5px] text-destructive"
              style={{ overflowWrap: "anywhere" }}
            >
              {session.configError}
            </div>
          )}
        </div>
      </div>
      </div>
    </>
  )
}

type SelectSessionConfigOption = Extract<SessionConfigOption, { type: "select" }>

function selectConfigOption(
  options: SessionConfigOption[] | undefined,
  category: "model" | "thought_level"
): SelectSessionConfigOption | undefined {
  return options?.find((option): option is SelectSessionConfigOption =>
    option.type === "select" && option.category === category
  )
}

function isConfigGroup(
  value: SessionConfigSelectOption | SessionConfigSelectGroup
): value is SessionConfigSelectGroup {
  return "group" in value
}

function SessionConfigControls({
  sessionId,
  session,
  disabled,
}: {
  sessionId: string
  session: SessionState
  disabled: boolean
}) {
  const { t } = useTranslation("panels")
  const setSessionConfigOption = useAgentStore((state) => state.setSessionConfigOption)
  const model = selectConfigOption(session.configOptions, "model")
  const effort = selectConfigOption(session.configOptions, "thought_level")
  if (!model && !effort && !session.mode) return null

  return (
    <div
      data-testid="agent-config-controls"
      data-layout="composer-toolbar"
      className="flex min-w-0 items-center gap-1"
    >
      {model && (
        <SessionConfigMenu
          option={model}
          label={t("agentZonePanel.modelConfig")}
          ariaLabel={t("agentZonePanel.modelConfigAria")}
          disabled={disabled}
          onChange={(value) => {
            void setSessionConfigOption(sessionId, model.id, value).catch(() => undefined)
          }}
        />
      )}
      {effort && (
        <SessionConfigMenu
          option={effort}
          label={t("agentZonePanel.effortConfig")}
          ariaLabel={t("agentZonePanel.effortConfigAria")}
          disabled={disabled}
          onChange={(value) => {
            void setSessionConfigOption(sessionId, effort.id, value).catch(() => undefined)
          }}
        />
      )}
      {/* mode 目前僅有 current_mode_update 單向通道（連線層無 setMode），故只顯示。 */}
      {session.mode && (
        <span
          title={t("agentZonePanel.modeLabel")}
          className="inline-flex h-6 shrink-0 items-center rounded-full border border-(--line-2) bg-(--yz-field) px-2.5 font-mono text-[10.5px] text-(--ink-2)"
        >
          {session.mode}
        </span>
      )}
    </div>
  )
}

function sessionConfigValueName(option: SelectSessionConfigOption): string {
  for (const item of option.options) {
    if (isConfigGroup(item)) {
      const selected = item.options.find((value) => value.value === option.currentValue)
      if (selected) return selected.name
      continue
    }
    if (item.value === option.currentValue) return item.name
  }
  return option.currentValue
}

// 2026-07-21 使用者回饋：model／thinking 選單改為可搜尋 combobox（Popover＋
// cmdk Command）。選中判斷用閉包中的 value.value，不吃 cmdk onSelect 參數——
// cmdk 會對 value 做正規化，直接回傳可能與 adapter 的 config value 大小寫不符。
function SessionConfigMenu({
  option,
  label,
  ariaLabel,
  disabled,
  onChange,
}: {
  option: SelectSessionConfigOption
  label: string
  ariaLabel: string
  disabled: boolean
  onChange: (value: string) => void
}) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const selectedName = sessionConfigValueName(option)

  const renderItem = (value: SessionConfigSelectOption) => {
    const checked = value.value === option.currentValue
    return (
      <CommandItem
        key={value.value}
        value={value.value}
        keywords={[value.name]}
        data-checked={checked || undefined}
        onSelect={() => {
          setOpen(false)
          if (!checked) onChange(value.value)
        }}
      >
        <Check aria-hidden="true" className={checked ? "opacity-100" : "opacity-0"} />
        {value.name}
      </CommandItem>
    )
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <InputGroupButton
          aria-label={ariaLabel}
          disabled={disabled}
          size="xs"
          variant="ghost"
          className="min-w-0 max-w-[min(13rem,42vw)] justify-start rounded-full border border-(--line-2) bg-(--yz-field) px-2.5 font-normal text-(--ink-2)"
        >
          <span className="shrink-0 text-[9.5px] font-semibold text-(--ink-3)">{label}</span>
          <span className="min-w-0 truncate font-mono text-[10.5px]">{selectedName}</span>
          <ChevronDown data-icon="inline-end" className="shrink-0 text-(--ink-4)" aria-hidden="true" />
        </InputGroupButton>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 max-w-[calc(100vw-2rem)] p-0">
        <Command>
          <CommandInput placeholder={t("agentZonePanel.configSearch", { name: option.name })} />
          <CommandList>
            <CommandEmpty>{t("agentZonePanel.configSearchEmpty")}</CommandEmpty>
            {option.options.map((item) => isConfigGroup(item) ? (
              <CommandGroup key={item.group} heading={item.name}>
                {item.options.map(renderItem)}
              </CommandGroup>
            ) : renderItem(item))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

// P5 雙 runtime：badge 判斷來源是 initialize.agentInfo.name（live 連線的事實），
// 不是 Settings 現值——restored 尚未續聊的 session 沒有 name、不顯示（誠實：
// 還不知道會連到哪個 runtime；Settings 切換只影響之後的 spawn）。
function piRuntimeBadgeKind(session: SessionState): "builtin" | "community" | null {
  if (session.agentId !== "pi") return null
  if (session.agentName === "yuzora-pi-acp") return "builtin"
  if (session.agentName === "pi-acp") return "community"
  return null
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
  const runtimeKind = piRuntimeBadgeKind(session)
  return (
    <div
      data-testid="agent-session-header"
      data-density="compact"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        minHeight: 43,
        padding: "8px 12px",
        borderBottom: "1px solid var(--line-1)",
        background: `linear-gradient(90deg, ${agent.bg}, transparent 56%)`,
      }}
    >
      <span
        data-testid="agent-avatar"
        aria-hidden="true"
        style={{
          width: 26,
          height: 26,
          borderRadius: 8,
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 11.5,
          fontWeight: 700,
          color: "var(--agent-badge-ink)",
          flex: "0 0 auto",
          background: agent.color,
        }}
      >
        {agent.short}
      </span>
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          alignItems: "center",
          gap: 6,
          overflow: "hidden",
        }}
      >
        <div
          style={{
            flex: 1,
            minWidth: 52,
            fontFamily: "var(--font-serif)",
            fontSize: 13.5,
            fontWeight: 600,
            color: "var(--ink-0)",
            letterSpacing: "-0.01em",
            lineHeight: 1.2,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {session.title}
        </div>
        <Badge
          variant="secondary"
          className="h-[19px] px-1.5 text-[9.5px]"
          style={{ background: agent.bg, color: agent.color }}
        >
          {agent.label}
        </Badge>
        <Badge
          variant="outline"
          className="h-[19px] border-[rgba(91,63,209,0.2)] bg-[rgba(91,63,209,0.08)] px-1.5 font-mono text-[9px] text-[#5b3fd1]"
        >
          ACP
        </Badge>
        {runtimeKind && (
          <Badge
            variant="outline"
            data-testid="agent-runtime-badge"
            title={t(runtimeKind === "builtin"
              ? "agentZonePanel.runtimeBuiltinTip"
              : "agentZonePanel.runtimeCommunityTip")}
            className="h-[19px] border-(--line-1) bg-(--paper-1) px-1.5 text-[9px] text-(--ink-2)"
          >
            {t(runtimeKind === "builtin"
              ? "agentZonePanel.runtimeBuiltin"
              : "agentZonePanel.runtimeCommunity")}
          </Badge>
        )}
        {session.usage && session.usage.size > 0 && <UsageChip usage={session.usage} />}
        {session.usage?.cost && <CostChip cost={session.usage.cost} />}
        {session.infoBanner && firstInfoLine(session.infoBanner) && (
          <InfoChip text={session.infoBanner} />
        )}
      </div>
      <Badge
        variant="secondary"
        className="h-[21px] gap-1.5 px-2 text-[10px]"
        style={{
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
      </Badge>
    </div>
  )
}

// <1000 原值；≥1000 一位小數 k；≥1_000_000 一位小數 M；整數位（.0）去尾（"200.0k"→"200k"）。
// round 後才選單位：k 欄位四捨五入達 1000.0 時（如 999,950）需進位改用 M，避免出現 "1000k"。
function scaledUsage(value: number, divisor: number, suffix: string): { scaled: number; text: string } {
  const scaled = Number((value / divisor).toFixed(1))
  const text = `${Number.isInteger(scaled) ? scaled : scaled.toFixed(1)}${suffix}`
  return { scaled, text }
}

function formatUsageCount(value: number): string {
  if (value < 1000) return String(value)
  if (value < 1_000_000) {
    const k = scaledUsage(value, 1_000, "k")
    if (k.scaled >= 1000) return scaledUsage(value, 1_000_000, "M").text
    return k.text
  }
  return scaledUsage(value, 1_000_000, "M").text
}

type UsageLevel = "normal" | "warn" | "danger"

function usageLevel(percent: number): UsageLevel {
  if (percent > 95) return "danger"
  if (percent > 80) return "warn"
  return "normal"
}

const USAGE_LEVEL_COLOR: Record<UsageLevel, string> = {
  normal: "var(--ink-3)",
  warn: "#9a6512",
  danger: "#b51f38",
}

function UsageRing({ percent, level }: { percent: number; level: UsageLevel }) {
  const size = 12
  const strokeWidth = 2
  const radius = (size - strokeWidth) / 2
  const circumference = 2 * Math.PI * radius
  const clamped = Math.max(0, Math.min(100, percent))
  const dashOffset = circumference * (1 - clamped / 100)
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true" style={{ flex: "0 0 auto" }}>
      <circle cx={size / 2} cy={size / 2} r={radius} fill="none" stroke="var(--line-1)" strokeWidth={strokeWidth} />
      <circle
        data-testid="agent-usage-ring"
        data-usage-level={level}
        cx={size / 2}
        cy={size / 2}
        r={radius}
        fill="none"
        stroke={USAGE_LEVEL_COLOR[level]}
        strokeWidth={strokeWidth}
        strokeDasharray={circumference}
        strokeDashoffset={dashOffset}
        strokeLinecap="round"
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
      />
    </svg>
  )
}

function UsageChip({ usage }: { usage: NonNullable<SessionState["usage"]> }) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const percent = usage.size > 0 ? (usage.used / usage.size) * 100 : 0
  const remaining = usage.size - usage.used
  const level = usageLevel(percent)

  useEffect(() => {
    if (!open) return
    function closeAndRestoreFocus(afterPointerDefault = false) {
      setOpen(false)
      if (afterPointerDefault) {
        globalThis.setTimeout(() => triggerRef.current?.focus(), 0)
      } else {
        triggerRef.current?.focus()
      }
    }
    function onKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault()
        closeAndRestoreFocus()
      }
    }
    function onMouseDown(event: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
        closeAndRestoreFocus(true)
      }
    }
    document.addEventListener("keydown", onKeyDown)
    document.addEventListener("mousedown", onMouseDown)
    return () => {
      document.removeEventListener("keydown", onKeyDown)
      document.removeEventListener("mousedown", onMouseDown)
    }
  }, [open])

  return (
    <div ref={containerRef} style={{ position: "relative", flex: "0 0 auto" }}>
      <button
        ref={triggerRef}
        type="button"
        data-testid="agent-usage-chip"
        aria-label={t("agentZonePanel.usageChipAria")}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls="agent-usage-popover"
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault()
            setOpen((value) => !value)
          }
        }}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 5,
          height: 18,
          padding: "0 7px",
          border: "1px solid var(--line-1)",
          borderRadius: "var(--r-pill)",
          background: "var(--paper-1)",
          fontFamily: "var(--font-mono)",
          fontSize: 9.5,
          fontWeight: 700,
          color: "var(--ink-3)",
          cursor: "pointer",
        }}
      >
        <UsageRing percent={percent} level={level} />
        {formatUsageCount(usage.used)} / {formatUsageCount(usage.size)}
      </button>
      {open && (
        <div
          data-testid="agent-usage-popover"
          id="agent-usage-popover"
          role="dialog"
          aria-label={t("agentZonePanel.usagePopoverAria")}
          style={{
            position: "absolute",
            top: 23,
            left: 0,
            zIndex: 20,
            width: 300,
            maxWidth: "calc(100cqw - 32px)",
            padding: "10px 12px",
            border: "1px solid var(--line-2)",
            borderRadius: 12,
            background: "var(--frost-light)",
            boxShadow: "var(--shadow-xl)",
            color: "var(--ink-2)",
            fontFamily: "var(--font-mono)",
            fontSize: 11,
            lineHeight: 1.5,
            whiteSpace: "normal",
          }}
        >
          <dl
            style={{
              display: "grid",
              gridTemplateColumns: "max-content minmax(0, 1fr)",
              gap: "4px 12px",
              margin: 0,
            }}
          >
            <dt>{t("agentZonePanel.usageUsed")}</dt>
            <dd data-testid="agent-usage-used" style={{ margin: 0, textAlign: "right" }}>
              {usage.used.toLocaleString()}
            </dd>
            <dt>{t("agentZonePanel.usageSize")}</dt>
            <dd data-testid="agent-usage-size" style={{ margin: 0, textAlign: "right" }}>
              {usage.size.toLocaleString()}
            </dd>
            <dt>{t("agentZonePanel.usageRemaining")}</dt>
            <dd data-testid="agent-usage-remaining" style={{ margin: 0, textAlign: "right" }}>
              {remaining.toLocaleString()}
            </dd>
            <dt>{t("agentZonePanel.usagePercent")}</dt>
            <dd data-testid="agent-usage-percent" style={{ margin: 0, textAlign: "right" }}>
              {String(percent)}%
            </dd>
            {usage.cost !== undefined && (
              <>
                <dt>{t("agentZonePanel.usageCost")}</dt>
                <dd data-testid="agent-usage-cost" style={{ margin: 0, textAlign: "right" }}>
                  {formatCostAmount(usage.cost.amount)} {usage.cost.currency}
                </dd>
              </>
            )}
          </dl>
        </div>
      )}
    </div>
  )
}

function formatCostAmount(amount: number): string {
  // Number#toString is the shortest round-trippable representation. In
  // particular it keeps ultra-micro finite values instead of rounding them to
  // zero at an arbitrary display precision; scientific notation is acceptable.
  return String(amount)
}

function CostChip({ cost }: { cost: NonNullable<NonNullable<SessionState["usage"]>["cost"]> }) {
  const { t } = useTranslation("panels")
  const amount = formatCostAmount(cost.amount)
  return (
    <span
      data-testid="agent-cost-chip"
      aria-label={t("agentZonePanel.costChipAria", { amount, currency: cost.currency })}
      style={{
        display: "inline-flex",
        alignItems: "center",
        height: 18,
        padding: "0 7px",
        borderRadius: "var(--r-pill)",
        background: "var(--mint-soft)",
        color: "#0f7a55",
        fontFamily: "var(--font-mono)",
        fontSize: 9.5,
        fontWeight: 700,
        whiteSpace: "nowrap",
        flex: "0 0 auto",
      }}
    >
      {amount} {cost.currency}
    </span>
  )
}

// 逐行去除開頭的 Markdown 記號（#/*/-）與行內的 ** 粗體記號，取第一個「去記號後仍非空」
// 的行；純分隔線（"---"）或標題記號單獨一行時會被去到空字串而略過，全部落空則回傳 ""。
function firstInfoLine(text: string): string {
  for (const candidate of text.split("\n")) {
    const stripped = candidate.trim().replace(/^[#*-]+\s*/, "").replace(/\*\*/g, "").trim()
    if (stripped.length > 0) return stripped
  }
  return ""
}

function InfoChip({ text }: { text: string }) {
  const { t } = useTranslation("panels")
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  // 換 session 時 text 會跟著變，殘留開啟的 popover 會顯示錯的 session 內容，故關閉。
  useEffect(() => {
    setOpen(false)
  }, [text])

  useEffect(() => {
    if (!open) return
    function onKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") setOpen(false)
    }
    function onMouseDown(event: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener("keydown", onKeyDown)
    document.addEventListener("mousedown", onMouseDown)
    return () => {
      document.removeEventListener("keydown", onKeyDown)
      document.removeEventListener("mousedown", onMouseDown)
    }
  }, [open])

  return (
    <div ref={containerRef} style={{ position: "relative", flex: "0 0 auto" }}>
      <button
        type="button"
        data-testid="agent-info-chip"
        aria-label={t("agentZonePanel.infoChipAria")}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        style={{
          display: "inline-flex",
          alignItems: "center",
          maxWidth: "40ch",
          height: 18,
          padding: "0 7px",
          borderRadius: "var(--r-pill)",
          fontSize: 10,
          fontWeight: 600,
          color: "var(--ink-3)",
          background: "var(--paper-2)",
          border: "1px solid var(--line-1)",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          cursor: "pointer",
        }}
      >
        {firstInfoLine(text)}
      </button>
      {open && (
        <div
          data-testid="agent-info-popover"
          role="dialog"
          aria-label={t("agentZonePanel.infoChipAria")}
          style={{
            position: "absolute",
            top: 22,
            left: 0,
            zIndex: 20,
            width: 380,
            maxHeight: 320,
            overflow: "auto",
            background: "var(--frost-light)",
            backdropFilter: "var(--blur-frost)",
            WebkitBackdropFilter: "var(--blur-frost)",
            border: "1px solid var(--line-2)",
            borderRadius: 14,
            boxShadow: "var(--shadow-xl)",
            padding: 12,
            fontSize: 11.5,
            lineHeight: 1.5,
            color: "var(--ink-2)",
            whiteSpace: "pre-wrap",
            animation: "yzpop 130ms var(--ease-spring)",
          }}
        >
          {text}
        </div>
      )}
    </div>
  )
}

// 判色依 session.agentId（"pi"/"claude"/"codex"）；agentLabel 只在沒有已知
// agentId（undefined 或 "custom"）時作為顯示文字 fallback，不參與判色。
function agentVisual(agentId: AgentPreset | undefined, label: string, fallback: string): AgentVisual {
  const known = agentId ? AGENT_TYPES[agentId] : undefined
  if (known) return known
  const display = agentDisplayName(agentId, label, fallback)
  return {
    label: display,
    short: display.slice(0, 1).toUpperCase(),
    color: CUSTOM_AGENT_VISUAL.colorVar,
    bg: CUSTOM_AGENT_VISUAL.softVar,
  }
}
