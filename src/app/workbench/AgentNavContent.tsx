import { MessagesSquare, Plus } from "lucide-react"
import type { CSSProperties } from "react"

import { EmptyState } from "@/app/workbench/EmptyState"
import type { AgentTone, SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const TONE_DOT: Record<AgentTone, CSSProperties["background"]> = {
  idle: "var(--ink-4)",
  run: "var(--yz-accent)",
  done: "#2bbf8a",
  wait: "#ffb23e",
  fail: "#e23b54",
}

/**
 * AgentZone mode nav content. Session row spacing and active treatment follow
 * dc.html L3121-L3129; tone colors mirror toneMap L3080-L3085.
 */
export function AgentNavContent() {
  const sessions = useAgentStore((state) => state.sessions)
  const activeSessionId = useAgentStore((state) => state.activeSessionId)
  const selectSession = useAgentStore((state) => state.selectSession)
  const newSession = useAgentStore((state) => state.newSession)
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const sessionEntries = Array.from(sessions.entries())

  function createSession() {
    void newSession(workspacePath ?? ".").catch(() => undefined)
  }

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
              onSelect={() => selectSession(sessionId)}
            />
          ))}
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={MessagesSquare}
            title="尚無 session"
            description="新增 session 以連接 Agent"
          />
          <span className="sr-only">尚無 session</span>
        </div>
      )}
      <NewSessionButton onClick={createSession} />
    </div>
  )
}

function NewSessionButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label="新增 session"
      onClick={onClick}
      className="flex h-[34px] w-full shrink-0 items-center justify-center gap-[6px] rounded-[10px] border border-dashed border-(--line-2) text-[12.5px] font-medium text-(--ink-3) transition-colors hover:border-(--yz-accent)/60 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] hover:text-(--yz-accent-ink)"
    >
      <Plus className="size-[14px]" aria-hidden="true" />
      新增 session
    </button>
  )
}

function SessionRow({
  sessionId,
  session,
  active,
  onSelect,
}: {
  sessionId: string
  session: SessionState
  active: boolean
  onSelect: () => void
}) {
  const metadata = session.model ? `${session.agentLabel} / ${session.model}` : session.agentLabel

  return (
    <button
      type="button"
      aria-current={active ? "page" : undefined}
      onClick={onSelect}
      className={
        "flex w-full items-center gap-[9px] rounded-[10px] px-[9px] py-[9px] text-left transition-colors duration-[120ms] hover:bg-(--yz-hover) " +
        (active
          ? "bg-(--yz-active) shadow-[inset_0_0_0_1px_var(--line-1)]"
          : "")
      }
    >
      <span
        data-testid={`agent-session-tone-${sessionId}`}
        aria-hidden="true"
        className="size-[8px] shrink-0 rounded-full"
        style={{ background: TONE_DOT[session.tone] }}
      />
      <span className="flex min-w-0 flex-1 flex-col gap-px">
        <span
          className={
            "truncate text-[12.5px] " +
            (active ? "font-semibold text-(--ink-0)" : "font-medium text-(--ink-1)")
          }
        >
          {session.title}
        </span>
        <span className="truncate font-mono text-[9.5px] text-(--ink-4)">
          {metadata}
        </span>
      </span>
    </button>
  )
}
