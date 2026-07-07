import { useEffect, useRef, useState } from "react"
import { ChevronDown, ChevronUp, Plus, TerminalSquare, X } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { loadTerminalSettings } from "@/app/workbench/SettingsDialog"
import { logUserAction } from "@/features/logs/userAction"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useTerminalStore, type TerminalSessionMeta, type TerminalSplitDirection } from "@/state/terminalStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { TerminalSession } from "@/terminal/TerminalSession"

const MIN_HEIGHT = 140
const MAX_HEIGHT = 480
const DEFAULT_HEIGHT = 228
const DEFAULT_COLS = 80
const DEFAULT_ROWS = 24
const MAX_PANES = 2

// The drawer never takes more than this share of the window height, so a
// remembered tall terminal can't swallow the editor when the window shrinks.
const MAX_WINDOW_FRACTION = 0.6

const maxDrawerHeight = () => Math.min(MAX_HEIGHT, Math.round(window.innerHeight * MAX_WINDOW_FRACTION))

const toolButtonClass =
  "flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg) disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-(--term-fg2)"

interface TerminalDrawerProps {
  visible: boolean
}

type TerminalSessionMetaWithArgs = TerminalSessionMeta & { shellArgs?: string[] }

function parseShellArgs(value: string): string[] | undefined {
  const args = value.trim().split(/\s+/).filter(Boolean)
  return args.length > 0 ? args : undefined
}

/**
 * Terminal drawer — design reference 5.5, a workspace-column sibling of the
 * main area shown for every mode. `visible` is the WorkspaceRail "Toggle
 * terminal" switch — the whole drawer (header included) animates in/out via
 * a grid-rows collapse and is completely hidden (inert) when off. That's a
 * separate concern from `expanded`, which is local: once visible, the
 * drawer's own header/chevron independently controls the resizable content
 * area (starts expanded). Sessions are user-created only; rendering or
 * visibility changes never open a pty by themselves.
 */
export function TerminalDrawer({ visible }: TerminalDrawerProps) {
  const { t } = useTranslation("terminal")
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const sessions = useTerminalStore((s) => s.sessions)
  const layout = useTerminalStore((s) => (workspacePath ? s.layouts[workspacePath] : undefined))
  const addSession = useTerminalStore((s) => s.addSession)
  const splitFrom = useTerminalStore((s) => s.splitFrom)
  const removeSession = useTerminalStore((s) => s.removeSession)
  const setActivePane = useTerminalStore((s) => s.setActivePane)
  const [expanded, setExpanded] = useState(true)
  const [height, setHeight] = useState(DEFAULT_HEIGHT)
  const [resizing, setResizing] = useState(false)
  const dragRef = useRef<{ startY: number; startHeight: number } | null>(null)
  const panes = layout?.panes ?? []
  const activePaneId = layout?.activePaneId ?? null
  const activePane = panes.find((pane) => pane.paneId === activePaneId) ?? panes[0]
  const activeSession = activePane ? sessions[activePane.sessionId] : undefined
  const canCreateSession = Boolean(workspacePath) && panes.length < MAX_PANES
  const canSplit = canCreateSession && Boolean(activePane)
  const canClose = Boolean(workspacePath && activeSession)

  // Keep the drawer within its share of the window as the window is resized.
  useEffect(() => {
    const clampToWindow = () => setHeight((h) => Math.min(h, maxDrawerHeight()))
    clampToWindow()
    window.addEventListener("resize", clampToWindow)
    return () => window.removeEventListener("resize", clampToWindow)
  }, [])

  const toggle = () => {
    const next = !expanded
    setExpanded(next)
    void logUserAction(
      "terminal_drawer_toggle",
      next ? "Expanded terminal drawer" : "Collapsed terminal drawer",
      { expanded: next }
    )
  }

  const createSessionMeta = (workspace: string): TerminalSessionMetaWithArgs => {
    const terminalSettings = loadTerminalSettings()
    const shell = terminalSettings.shellPath.trim()
    const shellArgs = parseShellArgs(terminalSettings.shellArgs)
    const sessionId =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `terminal-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    return {
      sessionId,
      title: `Terminal ${panes.length + 1}`,
      workspace,
      shell,
      shellArgs,
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
    }
  }

  const openSession = () => {
    if (!workspacePath || panes.length >= MAX_PANES) return
    const meta = createSessionMeta(workspacePath)
    addSession(workspacePath, meta)
    void logUserAction("terminal_new", "Open a new terminal")
  }

  const splitSession = (direction: TerminalSplitDirection) => {
    if (!workspacePath || !activePane || panes.length >= MAX_PANES) return
    const meta = createSessionMeta(workspacePath)
    splitFrom(workspacePath, activePane.paneId, meta, direction)
    void logUserAction(
      direction === "right" ? "terminal_split_right" : "terminal_split_down",
      direction === "right" ? "Split terminal right" : "Split terminal down"
    )
  }

  const closeSession = () => {
    if (!workspacePath || !activeSession) return
    removeSession(workspacePath, activeSession.sessionId)
    void logUserAction("terminal_close", "Close terminal session")
  }

  const selectPane = (paneId: string) => {
    if (!workspacePath) return
    setActivePane(workspacePath, paneId)
  }

  const logDockIntent = () => {
    void logUserAction("terminal_dock_editor", "Dock terminal into editor")
  }

  const onResizePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    dragRef.current = { startY: event.clientY, startHeight: height }
    setResizing(true)
  }

  const onResizePointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return
    const next = dragRef.current.startHeight + (dragRef.current.startY - event.clientY)
    setHeight(Math.min(maxDrawerHeight(), Math.max(MIN_HEIGHT, next)))
  }

  const onResizePointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    dragRef.current = null
    setResizing(false)
    event.currentTarget.releasePointerCapture(event.pointerId)
  }

  return (
    <div
      aria-hidden={!visible}
      inert={!visible}
      className="grid shrink-0 transition-[grid-template-rows,opacity] duration-[300ms] ease-(--ease-spring)"
      style={{ gridTemplateRows: visible ? "1fr" : "0fr", opacity: visible ? 1 : 0 }}
    >
      {/* Mirrors the nav sidebar's reveal (WorkspaceRail toggle): the panel
          springs to full height while fading in as one unit — a card popping
          out — rather than an accordion unfolding from an edge or expanding
          from its centre. opacity:0 while closed also stops the card's drop
          shadow from bleeding a hairline below the editor. No outer
          overflow-hidden so the open card keeps its shadow-lg. */}
      <div
        onContextMenu={contextMenuHandler("terminal")}
        className="flex min-h-0 flex-col overflow-hidden rounded-(--r-lg) border border-(--term-line) bg-(--term-bg) shadow-(--shadow-lg)"
      >
        {expanded && (
          <div
            onPointerDown={onResizePointerDown}
            onPointerMove={onResizePointerMove}
            onPointerUp={onResizePointerUp}
            title={t("terminalDrawer.dragToResize", { ns: "menus" })}
            className="flex h-[6px] shrink-0 cursor-row-resize items-center justify-center bg-(--term-bar)"
          >
            <span className="h-[3px] w-[34px] rounded-full bg-(--term-fg2) opacity-50" />
          </div>
        )}

        {/* Design reference: the whole header row toggles the drawer; the
            chevron stays a real button for keyboard/AT users. */}
        <div
          onClick={toggle}
          className="flex h-[38px] shrink-0 cursor-pointer items-center gap-[8px] bg-(--term-bar) px-[11px]"
        >
          <div className="flex min-w-0 flex-1 items-center gap-[8px] overflow-hidden">
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.9"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="shrink-0 text-(--term-fg2)"
              aria-hidden="true"
            >
              <path d="m5 8 4 4-4 4M13 16h6" />
            </svg>
            <span className="shrink-0 text-[12px] font-semibold text-(--term-fg)">
              {t("terminalDrawer.label", { ns: "menus" })}
            </span>
            {workspacePath && (
              <span className="min-w-0 flex-1 truncate text-right font-mono text-[10px] text-(--term-fg2)">
                {workspacePath}
              </span>
            )}
          </div>

          <div className="flex shrink-0 items-center gap-[6px]">
            <button
              type="button"
              title={t("terminalDrawer.splitRightTitle", { ns: "menus" })}
              disabled={!canSplit}
              onClick={(event) => {
                event.stopPropagation()
                splitSession("right")
              }}
              className={toolButtonClass}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.9"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <path d="M13 4v16" />
              </svg>
            </button>
            <button
              type="button"
              title={t("terminalDrawer.splitDownTitle", { ns: "menus" })}
              disabled={!canSplit}
              onClick={(event) => {
                event.stopPropagation()
                splitSession("down")
              }}
              className={toolButtonClass}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.9"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <path d="M3 13h18" />
              </svg>
            </button>
            <button
              type="button"
              title={t("terminalDrawer.newTerminalTitle", { ns: "menus" })}
              disabled={!canCreateSession}
              onClick={(event) => {
                event.stopPropagation()
                openSession()
              }}
              className={toolButtonClass}
            >
              <Plus className="size-[13px]" aria-hidden="true" />
            </button>
            <button
              type="button"
              title={t("terminalDrawer.closeTerminalTitle", { ns: "menus" })}
              disabled={!canClose}
              onClick={(event) => {
                event.stopPropagation()
                closeSession()
              }}
              className={toolButtonClass}
            >
              <X className="size-[13px]" aria-hidden="true" />
            </button>
            <div className="mx-[1px] h-[16px] w-px shrink-0 bg-(--term-line)" />
            <button
              type="button"
              title={t("terminalDrawer.dockTitle", { ns: "menus" })}
              onClick={(event) => {
                event.stopPropagation()
                logDockIntent()
              }}
              className={toolButtonClass}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.9"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <path d="M15 4v16" />
              </svg>
            </button>
            <button
              type="button"
              aria-expanded={expanded}
              aria-label={
                expanded
                  ? t("terminalDrawer.collapseAriaLabel", { ns: "menus" })
                  : t("terminalDrawer.expandAriaLabel", { ns: "menus" })
              }
              onClick={(event) => {
                event.stopPropagation()
                toggle()
              }}
              className={toolButtonClass}
            >
              {expanded ? (
                <ChevronUp className="size-[15px]" aria-hidden="true" />
              ) : (
                <ChevronDown className="size-[15px]" aria-hidden="true" />
              )}
            </button>
          </div>
        </div>

        <div
          aria-hidden={!expanded}
          inert={!expanded}
          className="yzs overflow-y-auto font-mono"
          style={{
            height: expanded ? height : 0,
            opacity: expanded ? 1 : 0,
            // Dragging the resize handle updates `height` on every pointermove;
            // an active transition would make the box lag behind the pointer
            // instead of tracking it 1:1, so it's suppressed while resizing.
            transition: resizing
              ? "none"
              : "height 220ms var(--ease-spring), opacity 160ms var(--ease-out)",
          }}
        >
          {panes.length === 0 ? (
            <div className="flex h-full items-center justify-center">
              <EmptyState
                icon={TerminalSquare}
                title={t("noSessions")}
                description={t("emptyDescription")}
                tone="terminal"
              />
            </div>
          ) : (
            <div
              className={`flex h-full min-h-0 ${layout?.splitDirection === "down" ? "flex-col" : "flex-row"}`}
              data-testid="terminal-pane-grid"
            >
              {panes.map((pane, index) => {
                const session = sessions[pane.sessionId]
                if (!session || !workspacePath) return null
                const active = pane.paneId === activePaneId
                const shellArgs = (session as TerminalSessionMetaWithArgs).shellArgs
                const dividerClass =
                  layout?.splitDirection === "down"
                    ? index > 0
                      ? "border-t border-(--term-line)"
                      : ""
                    : index > 0
                      ? "border-l border-(--term-line)"
                      : ""

                return (
                  <div
                    key={pane.paneId}
                    data-testid={`terminal-pane-${pane.paneId}`}
                    onClick={() => selectPane(pane.paneId)}
                    className={`min-h-0 min-w-0 flex-1 ${dividerClass} ${active ? "ring-1 ring-inset ring-(--term-blue)" : ""}`}
                  >
                    <TerminalSession
                      workspace={workspacePath}
                      sessionId={session.sessionId}
                      shell={session.shell || null}
                      shellArgs={shellArgs}
                      active={active}
                      onExit={() => removeSession(workspacePath, session.sessionId)}
                    />
                  </div>
                )
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
