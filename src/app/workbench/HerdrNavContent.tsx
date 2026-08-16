import { Bot, Plus } from "lucide-react"
import type { CSSProperties } from "react"
import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { ScrollArea } from "@/components/ui/scroll-area"
import type { HerdrAgentInfo, HerdrAgentStatus, HerdrNamedSession } from "@/lib/herdrTypes"
import { cn } from "@/lib/utils"
import { openCreatedHerdrTabAndRequestName } from "@/lib/herdrTabActions"
import { showActionError } from "@/lib/actionFeedback"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useHerdrStore } from "@/state/herdrStore"
import { useUiStore } from "@/state/uiStore"

const STATUS_DOT: Record<HerdrAgentStatus, CSSProperties["background"]> = {
  idle: "var(--ink-4)",
  working: "var(--yz-run)",
  blocked: "#ffb23e",
  done: "#2bbf8a",
  unknown: "var(--ink-4)"
}

/**
 * ADE ProjectNav: named Session tabs + Agents only.
 * Spaces live on WorkspaceRail. Agent activation is transaction-like.
 */
export function HerdrNavContent() {
  const { t } = useTranslation("workbench")
  const sessions = useHerdrStore((s) => s.sessions)
  const selectedSessionName = useHerdrStore((s) => s.selectedSessionName)
  const selectSession = useHerdrStore((s) => s.selectSession)
  const connectionState = useHerdrStore((s) => s.connectionState)
  const errorMessage = useHerdrStore((s) => s.errorMessage)
  const snapshot = useHerdrStore((s) => s.snapshot)
  const selectedSpaceId = useHerdrStore((s) => s.selectedSpaceId)
  const createTerminalInSelectedSpace = useHerdrStore((s) => s.createTerminalInSelectedSpace)
  const canCreateTerminal = useHerdrStore((s) => s.canCreateTerminal())
  const canMutate = useHerdrStore((s) => s.canMutateSelectedSession())
  const canFocusTab = useHerdrStore((s) => s.canFocusSelectedTab())
  const createBlockedReason = useHerdrStore((s) => s.createTerminalBlockedReason())
  const activateAgent = useHerdrStore((s) => s.activateAgent)
  const setMode = useUiStore((s) => s.setMode)
  const [creating, setCreating] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const attentionByKey = useHerdrStore((s) => s.attentionByKey)
  const attentionItems = useMemo(() => {
    const selected = selectedSessionName
    return Array.from(attentionByKey.values())
      .filter((item) => {
        if (selected && item.sessionName !== selected) return false
        if (item.kind === "done" && item.seen) return false
        return true
      })
      .sort((a, b) => b.updatedAt - a.updatedAt)
  }, [attentionByKey, selectedSessionName])

  const agents = snapshot?.agents ?? []
  const herdrSessionId = selectedSessionName ?? snapshot?.herdrSessionId
  const stopped = connectionState === "stopped"
  const createDisabled =
    !selectedSpaceId || creating || !herdrSessionId || !canCreateTerminal || stopped
  const visibleError = actionError ?? errorMessage

  const onSelectSession = (session: HerdrNamedSession) => {
    // HerdrBridge is the single focus-restoration owner so a user-closed page
    // is not reopened while the runtime focus key remains unchanged.
    void selectSession(session.name)
  }

  const openAgent = async (agent: HerdrAgentInfo) => {
    if (stopped || !canMutate) return
    setActionError(null)
    const result = await activateAgent(agent)
    if (!result.ok) {
      if (result.cancelled) return
      if (result.error) setActionError(result.error)
    }
  }

  const onCreateTerminal = async () => {
    if (creating || createDisabled || !herdrSessionId) return
    setCreating(true)
    setActionError(null)
    try {
      const created = await createTerminalInSelectedSpace()
      if (!created) {
        const reason =
          useHerdrStore.getState().errorMessage ??
          createBlockedReason ??
          t("herdrNav.createFailedUnknown")
        setActionError(t("herdrNav.createFailed", { reason }))
        return
      }
      setActionError(null)
      setMode("ade")
      try {
        await openCreatedHerdrTabAndRequestName({
          sessionName: created.herdrSessionId,
          workspaceId: created.workspaceId,
          terminalId: created.terminalId,
          title: created.title,
          paneId: created.paneId,
          tabId: created.tabId
        })
      } catch (error) {
        await showActionError(t("herdrNav.renameTabAction"), error)
      }
    } finally {
      setCreating(false)
    }
  }

  if (
    (connectionState === "unsupported" || connectionState === "error") &&
    !snapshot &&
    sessions.length === 0
  ) {
    return (
      <div className="flex h-full flex-col gap-[10px]">
        <EmptyState
          icon={Bot}
          title={t("herdrNav.unavailableTitle")}
          description={errorMessage ?? t("herdrNav.unavailableDescription")}
        />
      </div>
    )
  }

  if (
    (connectionState === "connecting" || connectionState === "idle") &&
    !snapshot &&
    sessions.length === 0
  ) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-(--ink-3)">
        {t("herdrNav.connecting")}
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col gap-[10px]">
      {sessions.length > 0 && (
        <div
          role="tablist"
          aria-label={t("herdrNav.sessionsHeading")}
          className="flex shrink-0 flex-wrap gap-[6px] px-[2px]"
        >
          {sessions.map((session) => {
            const selected = session.name === selectedSessionName
            return (
              <button
                key={session.name}
                type="button"
                role="tab"
                data-testid={`herdr-session-${session.name}`}
                aria-selected={selected}
                title={
                  session.running
                    ? session.socketPath
                    : t("herdrNav.sessionStoppedTitle", { name: session.name })
                }
                onClick={() => onSelectSession(session)}
                className={cn(
                  "rounded-full border px-[10px] py-[4px] text-[11px] font-medium transition-colors",
                  selected
                    ? "border-(--yz-accent)/50 bg-(--yz-active) text-(--ink-0)"
                    : "border-(--line-2) text-(--ink-3) hover:bg-(--yz-hover)",
                  !session.running && "opacity-70"
                )}
              >
                <span>{session.name}</span>
                {!session.running && (
                  <span className="ml-[4px] text-[10px] text-(--ink-4)">
                    {t("herdrNav.stoppedBadge")}
                  </span>
                )}
              </button>
            )
          })}
        </div>
      )}

      {visibleError && <ErrorBanner message={visibleError} />}

      {stopped && (
        <div
          role="status"
          data-testid="herdr-session-stopped"
          className="shrink-0 rounded-[10px] border border-(--line-2) bg-(--yz-active) px-[10px] py-[8px] text-[12px] text-(--ink-2)"
        >
          {t("herdrNav.sessionStopped", {
            name: selectedSessionName ?? "session"
          })}
        </div>
      )}

      <ScrollArea className="min-h-0 flex-1" viewportClassName="py-[4px]">
        {attentionItems.length > 0 && (
          <>
            <SectionLabel>{t("herdrNav.attentionHeading")}</SectionLabel>
            {attentionItems.map((item) => {
              const agent = agents.find((candidate) => candidate.paneId === item.paneId) ?? null
              return (
                <button
                  key={item.key}
                  type="button"
                  data-testid={`herdr-attention-${item.paneId}`}
                  className="mb-[4px] flex w-full items-center gap-[8px] rounded-[10px] px-[8px] py-[7px] text-left text-(--ink-2) transition-colors hover:bg-(--yz-hover)"
                  onClick={() => {
                    if (agent) void openAgent(agent)
                  }}
                >
                  <span className="size-[7px] shrink-0 rounded-full bg-[#ffb23e]" aria-hidden="true" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[12.5px] font-medium">
                      {item.title ?? item.displayAgent ?? item.paneId}
                    </span>
                    <span className="block truncate text-[10px] text-(--ink-4)">
                      {item.kind} · {item.agentStatus}
                    </span>
                  </span>
                </button>
              )
            })}
          </>
        )}
        <SectionLabel>{t("herdrNav.agentsHeading")}</SectionLabel>
        {agents.length === 0 ? (
          <p className="px-[8px] py-[6px] text-[12px] text-(--ink-4)">
            {stopped ? t("herdrNav.stoppedNoAgents") : t("herdrNav.noAgents")}
          </p>
        ) : (
          agents.map((agent) => (
            <AgentRow
              key={`${agent.sessionName ?? herdrSessionId}:${agent.id}`}
              agent={agent}
              sessionName={herdrSessionId}
              disabled={
                stopped ||
                !agent.terminalId ||
                (agent.tabId ? !canFocusTab : !canMutate)
              }
              onSelect={() => void openAgent(agent)}
            />
          ))
        )}
      </ScrollArea>
      <CreateTerminalButton
        onClick={() => void onCreateTerminal()}
        disabled={createDisabled}
        reason={!canCreateTerminal || stopped ? createBlockedReason : null}
      />
    </div>
  )
}

function ErrorBanner({ message }: { message: string }) {
  return (
    <div
      role="alert"
      data-testid="herdr-nav-error"
      className="shrink-0 rounded-[10px] border border-(--line-2) bg-(--yz-active) px-[10px] py-[8px] text-[12px] text-(--ink-2)"
    >
      {message}
    </div>
  )
}

function SectionLabel({
  children,
  className
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        "px-[8px] pb-[4px] text-[10px] font-semibold tracking-[0.09em] text-(--ink-4) uppercase",
        className
      )}
    >
      {children}
    </div>
  )
}

function AgentRow({
  agent,
  disabled,
  onSelect,
  sessionName
}: {
  agent: HerdrAgentInfo
  disabled?: boolean
  onSelect: () => void
  sessionName?: string | null
}) {
  const spaceLabel = agent.spaceLabel ?? agent.workspaceId
  const resolvedSession = agent.sessionName ?? sessionName ?? ""
  return (
      <button
        type="button"
        data-testid={`herdr-agent-${agent.id}`}
        disabled={disabled}
        onClick={onSelect}
        onContextMenu={
          agent.paneId
            ? contextMenuHandler({
                kind: "herdrPane",
                sessionName: resolvedSession,
                paneId: agent.paneId,
                terminalId: agent.terminalId ?? null,
                tabId: agent.tabId ?? null,
                workspaceId: agent.workspaceId,
                label: agent.title ?? agent.name,
                focusedPaneId: null
              })
            : undefined
        }
        className={cn(
          "flex w-full min-w-0 items-center gap-[8px] rounded-[10px] px-[8px] py-[7px] text-left text-(--ink-2) transition-colors hover:bg-(--yz-hover)",
          "disabled:pointer-events-none disabled:opacity-40"
        )}
      >
        <span
          aria-hidden="true"
          className="size-[7px] shrink-0 rounded-full"
          style={{ background: STATUS_DOT[agent.status] }}
        />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[12.5px] font-medium">
            {agent.title ?? agent.name}
          </span>
          <span className="block truncate text-[10px] text-(--ink-4)">{spaceLabel}</span>
        </span>
        <span className="shrink-0 font-mono text-[10px] text-(--ink-4)">{agent.status}</span>
      </button>
  )
}

function CreateTerminalButton({
  onClick,
  disabled,
  reason
}: {
  onClick: () => void
  disabled?: boolean
  reason?: string | null
}) {
  const { t } = useTranslation("workbench")
  const title = reason
    ? t("herdrNav.createUnavailable", { reason })
    : t("herdrNav.newTerminal")
  return (
    <button
      type="button"
      data-testid="herdr-create-terminal"
      aria-label={title}
      title={title}
      onClick={onClick}
      disabled={disabled}
      className="flex h-[34px] w-full shrink-0 items-center justify-center gap-[6px] rounded-[10px] border border-dashed border-(--line-2) text-[12.5px] font-medium text-(--ink-3) transition-colors hover:border-(--yz-accent)/60 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] hover:text-(--yz-accent-ink) disabled:pointer-events-none disabled:opacity-50"
    >
      <Plus className="size-[14px]" aria-hidden="true" />
      {t("herdrNav.newTerminal")}
    </button>
  )
}
