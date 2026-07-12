import { MessagesSquare, Plus } from "lucide-react"
import type { CSSProperties, KeyboardEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { AgentPickerPopover } from "@/app/workbench/AgentPickerPopover"
import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { AGENT_VISUALS, agentDisplayName } from "@/lib/agentPresets"
import { firstAbsolutePath } from "@/lib/paths"
import type { AgentTone, SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { normalizeWorkspacePath } from "@/state/recentWorkspaces"
import { useWorkspaceStore } from "@/state/workspaceStore"

const TONE_DOT: Record<AgentTone, CSSProperties["background"]> = {
  idle: "var(--ink-4)",
  run: "var(--yz-run)",
  done: "#2bbf8a",
  wait: "#ffb23e",
  fail: "#e23b54",
}

// custom／unknown／undefined agentId 時顯示名的 fallback 字面值；與
// AgentZonePanel 的 agentVisual（t("agentZonePanel.agentFallback")，en/zh-TW
// 皆為 "Agent"）同一語意，維持兩處一致。
const AGENT_NAME_FALLBACK = "Agent"

// Row badge glyph/color by agentId; unknown/custom falls back to the
// neutral --agent-custom token with a first-letter glyph of session.agentLabel
// (mirrors AgentZonePanel's agentVisual fallback logic via AGENT_VISUALS).
function agentBadgeColor(agentId: SessionState["agentId"]): string {
  const known = agentId && agentId !== "custom" ? AGENT_VISUALS[agentId] : undefined
  return known?.colorVar ?? "var(--agent-custom)"
}
function agentBadgeGlyph(agentId: SessionState["agentId"], agentName: string): string {
  const known = agentId && agentId !== "custom" ? AGENT_VISUALS[agentId] : undefined
  if (known) return known.glyph
  return agentName.slice(0, 1).toUpperCase() || "?"
}

/**
 * AgentZone mode nav content. Session row spacing and active treatment follow
 * dc.html L3121-L3129; tone colors mirror toneMap L3080-L3085.
 */
export function AgentNavContent() {
  const { t } = useTranslation("workbench")
  const sessions = useAgentStore((state) => state.sessions)
  const activeSessionId = useAgentStore((state) => state.activeSessionId)
  const continueSession = useAgentStore((state) => state.continueSession)
  const pendingNewSession = useAgentStore((state) => state.pendingNewSession)
  const renamingSessionId = useAgentStore((state) => state.renamingSessionId)
  const setSessionAlias = useAgentStore((state) => state.setSessionAlias)
  const endRenameSession = useAgentStore((state) => state.endRenameSession)
  const confirmRemoveRequest = useAgentStore((state) => state.confirmRemoveRequest)
  const respondRemoveSessionConfirm = useAgentStore((state) => state.respondRemoveSessionConfirm)
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const cwd = firstAbsolutePath(workspacePath)
  const sessionEntries = Array.from(sessions.entries()).filter(
    ([, session]) =>
      cwd != null &&
      session.cwd != null &&
      normalizeWorkspacePath(session.cwd) === normalizeWorkspacePath(cwd)
  )
  const [pickerOpen, setPickerOpen] = useState(false)
  const confirmRemoveSession = confirmRemoveRequest
    ? sessions.get(confirmRemoveRequest.sessionId)
    : undefined

  return (
    <div className="flex h-full flex-col gap-[10px]">
      {sessionEntries.length > 0 ? (
        <div className="flex-1 overflow-y-auto py-[4px]">
          {sessionEntries.map(([sessionId, session]) => (
            <SessionRow
              key={sessionId}
              sessionId={sessionId}
              session={session}
              active={sessionId === activeSessionId}
              renaming={renamingSessionId === sessionId}
              onSelect={() => void continueSession(sessionId).catch(() => undefined)}
              onRenameCommit={(alias) => {
                setSessionAlias(sessionId, alias)
                endRenameSession()
              }}
              onRenameCancel={endRenameSession}
            />
          ))}
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={MessagesSquare}
            title={t("agentNav.emptyTitle")}
            description={t("agentNav.emptyDescription")}
          />
          <span className="sr-only">{t("agentNav.emptyTitle")}</span>
        </div>
      )}
      <div className="relative flex shrink-0 items-center gap-[8px]">
        {pickerOpen && cwd && (
          <AgentPickerPopover cwd={cwd} onClose={() => setPickerOpen(false)} />
        )}
        <NewSessionButton
          onClick={() => setPickerOpen((value) => !value)}
          disabled={pendingNewSession || !cwd}
        />
      </div>
      <RemoveSessionDialog
        session={confirmRemoveSession}
        onConfirm={() => respondRemoveSessionConfirm(true)}
        onCancel={() => respondRemoveSessionConfirm(false)}
      />
    </div>
  )
}

// Sessions context menu 的移除確認 modal（沿 ConfirmDialogHost 的 Dialog 結構，
// 但語意單純為 confirm/cancel——沒有 save 分支）。開關綁 agentStore 的
// confirmRemoveRequest：session 不存在（理論上不會發生，menu availability 已擋）時
// 視同關閉。
function RemoveSessionDialog({
  session,
  onConfirm,
  onCancel
}: {
  session: SessionState | undefined
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation("workbench")
  return (
    <Dialog open={session !== undefined} onOpenChange={(open) => { if (!open) onCancel() }}>
      {session && (
        <DialogContent showCloseButton={false} className="sm:max-w-[420px]">
          <DialogHeader>
            <DialogTitle>{t("agentNav.removeSessionTitle")}</DialogTitle>
            <DialogDescription>
              {t("agentNav.removeSessionDescription", { name: session.title })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={onCancel}>
              {t("agentNav.removeSessionCancel")}
            </Button>
            <Button variant="destructive" onClick={onConfirm}>
              {t("agentNav.removeSessionConfirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      )}
    </Dialog>
  )
}

function NewSessionButton({ onClick, disabled }: { onClick: () => void; disabled?: boolean }) {
  const { t } = useTranslation("workbench")
  return (
    <button
      type="button"
      aria-label={t("agentNav.newSession")}
      onClick={onClick}
      disabled={disabled}
      className="flex h-[34px] w-full shrink-0 items-center justify-center gap-[6px] rounded-[10px] border border-dashed border-(--line-2) text-[12.5px] font-medium text-(--ink-3) transition-colors hover:border-(--yz-accent)/60 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] hover:text-(--yz-accent-ink) disabled:pointer-events-none disabled:opacity-50"
    >
      <Plus className="size-[14px]" aria-hidden="true" />
      {t("agentNav.newSession")}
    </button>
  )
}

function SessionRow({
  sessionId,
  session,
  active,
  renaming,
  onSelect,
  onRenameCommit,
  onRenameCancel,
}: {
  sessionId: string
  session: SessionState
  active: boolean
  renaming: boolean
  onSelect: () => void
  onRenameCommit: (alias: string) => void
  onRenameCancel: () => void
}) {
  const agentName = agentDisplayName(session.agentId, session.agentLabel, AGENT_NAME_FALLBACK)
  const metadata = session.model
    ? `${agentName} · ${session.agentLabel} / ${session.model}`
    : `${agentName} · ${session.agentLabel}`
  const rowClassName =
    "flex w-full items-center gap-[9px] rounded-[10px] px-[9px] py-[9px] text-left transition-colors duration-[120ms] hover:bg-(--yz-hover) " +
    (active ? "bg-(--yz-active) shadow-[inset_0_0_0_1px_var(--line-1)]" : "")

  const toneDot = (
    <span
      data-testid={`agent-session-tone-${sessionId}`}
      aria-hidden="true"
      className="size-[8px] shrink-0 rounded-full"
      style={{ background: TONE_DOT[session.tone] }}
    />
  )
  const badge = (
    <span
      data-testid={`agent-session-badge-${sessionId}`}
      aria-hidden="true"
      className="flex size-[16px] shrink-0 items-center justify-center rounded-[5px] text-[9px] font-bold text-(--agent-badge-ink)"
      style={{ background: agentBadgeColor(session.agentId) }}
    >
      {agentBadgeGlyph(session.agentId, agentName)}
    </span>
  )
  const metadataLine = (
    <span className="truncate font-mono text-[9.5px] text-(--ink-4)">{metadata}</span>
  )

  function handleRenameKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault()
      onRenameCommit(event.currentTarget.value)
    } else if (event.key === "Escape") {
      event.preventDefault()
      onRenameCancel()
    }
  }

  return (
    // 外層 div 掛右鍵選單（沿 TabBar 的 span+button 結構）——renaming 時內層改
    // input，不能巢在 <button> 裡（HTML 不允許互動元素巢狀），故 renaming 分支
    // 不用 button。
    <div onContextMenu={contextMenuHandler({ kind: "agentSession", sessionId })}>
      {renaming ? (
        <div className={rowClassName}>
          {toneDot}
          {badge}
          <span className="flex min-w-0 flex-1 flex-col gap-px">
            <Input
              data-testid={`agent-session-rename-${sessionId}`}
              defaultValue={session.title}
              autoFocus
              onFocus={(event) => event.currentTarget.select()}
              onKeyDown={handleRenameKeyDown}
              onBlur={(event) => onRenameCommit(event.currentTarget.value)}
              className="h-[20px] rounded-[4px] px-[4px] py-0 text-[12.5px] font-medium"
            />
            {metadataLine}
          </span>
        </div>
      ) : (
        <button
          type="button"
          aria-current={active ? "page" : undefined}
          onClick={onSelect}
          className={rowClassName}
        >
          {toneDot}
          {badge}
          <span className="flex min-w-0 flex-1 flex-col gap-px">
            <span
              className={
                "truncate text-[12.5px] " +
                (active ? "font-semibold text-(--ink-0)" : "font-medium text-(--ink-1)")
              }
            >
              {session.title}
            </span>
            {metadataLine}
          </span>
        </button>
      )}
    </div>
  )
}
