import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
} from "react"
import { ChevronDown, ChevronUp, Plus, TerminalSquare, X } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { SplitRatioIndicator } from "@/app/workbench/SplitRatioIndicator"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { MAX_PANES, useTerminalStore, type TerminalSplitDirection } from "@/state/terminalStore"
import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { TerminalSession } from "@/terminal/TerminalSession"
import {
  closeTerminal,
  createTerminalSessionMeta,
  splitTerminal,
  type TerminalSessionMetaWithArgs,
} from "@/terminal/terminalCommands"

const ACTIVE_GAP = 10
const TERMINAL_CONTENT_MIN_HEIGHT = 140
const TERMINAL_HANDLE_HEIGHT = 6
const TERMINAL_HEADER_HEIGHT = 38
const TERMINAL_FIXED_CHROME_HEIGHT = TERMINAL_HANDLE_HEIGHT + TERMINAL_HEADER_HEIGHT
const KEYBOARD_STEP = 0.02
const KEYBOARD_LARGE_STEP = 0.1

const toolButtonClass =
  "flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg) disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-(--term-fg2)"

interface TerminalDrawerProps {
  visible: boolean
  containerRef: RefObject<HTMLElement | null>
  mainSurfaceMinHeight: number
}

interface TerminalRatioBounds {
  min: number
  max: number
  canResize: boolean
}

interface TerminalGeometry extends TerminalRatioBounds {
  distributableHeight: number
  allocation: number
  contentHeight: number
  effectiveRatio: number
}

interface DragState {
  pointerId: number
  containerBottom: number
  distributableHeight: number
  initialRatio: number
  ratio: number
  moved: boolean
  canCommit: boolean
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function ratioBounds(
  containerHeight: number,
  mainSurfaceMinHeight: number
): TerminalRatioBounds & { distributableHeight: number } {
  const distributableHeight = Math.max(0, containerHeight - ACTIVE_GAP)
  if (distributableHeight === 0) {
    return { distributableHeight, min: 0, max: 0, canResize: false }
  }

  const maxAllocation = Math.max(0, distributableHeight - mainSurfaceMinHeight)
  const preferredMinAllocation = TERMINAL_FIXED_CHROME_HEIGHT + TERMINAL_CONTENT_MIN_HEIGHT
  // If the container cannot satisfy both the 140px content minimum and the
  // main floor, the floor wins and Terminal receives all remaining safe space.
  // This shrinks only its content and leaves the stored preference untouched.
  const minAllocation = Math.min(maxAllocation, preferredMinAllocation)
  const min = minAllocation / distributableHeight
  const max = maxAllocation / distributableHeight
  return {
    distributableHeight,
    min,
    max,
    canResize: maxAllocation > minAllocation,
  }
}

function terminalGeometry(
  ratio: number,
  containerHeight: number,
  mainSurfaceMinHeight: number
): TerminalGeometry {
  const bounds = ratioBounds(containerHeight, mainSurfaceMinHeight)
  if (bounds.distributableHeight === 0) {
    return { ...bounds, allocation: 0, contentHeight: 0, effectiveRatio: 0 }
  }

  const effectiveRatio = clamp(ratio, bounds.min, bounds.max)
  const allocation = effectiveRatio * bounds.distributableHeight
  return {
    ...bounds,
    allocation,
    contentHeight: Math.max(0, allocation - TERMINAL_FIXED_CHROME_HEIGHT),
    effectiveRatio,
  }
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
export function TerminalDrawer({
  visible,
  containerRef,
  mainSurfaceMinHeight,
}: TerminalDrawerProps) {
  const { t } = useTranslation("terminal")
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const sessions = useTerminalStore((s) => s.sessions)
  const layout = useTerminalStore((s) => (workspacePath ? s.layouts[workspacePath] : undefined))
  const addSession = useTerminalStore((s) => s.addSession)
  const removeSession = useTerminalStore((s) => s.removeSession)
  const setActivePane = useTerminalStore((s) => s.setActivePane)
  const storedRatio = useWorkbenchLayoutStore((state) => {
    if (state.terminalRatioScope === "workspace" && workspacePath) {
      return state.terminalWorkspaceRatios[workspacePath] ?? state.terminalGlobalRatio
    }
    return state.terminalGlobalRatio
  })
  const setTerminalRatio = useWorkbenchLayoutStore((state) => state.setTerminalRatio)
  const [expanded, setExpanded] = useState(true)
  const [containerHeight, setContainerHeight] = useState(0)
  const [transientRatio, setTransientRatio] = useState<number | null>(null)
  const [resizing, setResizing] = useState(false)
  const [geometryTransitionSuppressed, setGeometryTransitionSuppressed] = useState(false)
  const dragRef = useRef<DragState | null>(null)
  const measuredContainerHeightRef = useRef(0)
  const transitionResetFrameRef = useRef<number | null>(null)
  const previousMainSurfaceMinHeightRef = useRef(mainSurfaceMinHeight)
  const panes = layout?.panes ?? []
  const activePaneId = layout?.activePaneId ?? null
  const activePane = panes.find((pane) => pane.paneId === activePaneId)
  const activeSession = activePane ? sessions[activePane.sessionId] : undefined
  const canCreateSession = Boolean(workspacePath) && panes.length < MAX_PANES
  const canSplit = canCreateSession && Boolean(activePane)
  const canClose = Boolean(workspacePath && activeSession)

  const suppressGeometryTransition = useCallback(() => {
    setGeometryTransitionSuppressed(true)
    if (transitionResetFrameRef.current !== null) {
      cancelAnimationFrame(transitionResetFrameRef.current)
    }
    transitionResetFrameRef.current = requestAnimationFrame(() => {
      transitionResetFrameRef.current = requestAnimationFrame(() => {
        transitionResetFrameRef.current = null
        setGeometryTransitionSuppressed(false)
      })
    })
  }, [])

  useLayoutEffect(() => () => {
    if (transitionResetFrameRef.current !== null) {
      cancelAnimationFrame(transitionResetFrameRef.current)
    }
  }, [])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const updateHeight = (height: number) => {
      if (measuredContainerHeightRef.current === height) return
      measuredContainerHeightRef.current = height
      dragRef.current = null
      setTransientRatio(null)
      setResizing(false)
      suppressGeometryTransition()
      setContainerHeight(height)
    }

    updateHeight(container.getBoundingClientRect().height)
    const observer = new ResizeObserver((entries) => {
      const entry = entries.find((candidate) => candidate.target === container) ?? entries[0]
      if (entry) updateHeight(entry.contentRect.height)
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [containerRef, suppressGeometryTransition])

  useLayoutEffect(() => {
    if (previousMainSurfaceMinHeightRef.current === mainSurfaceMinHeight) return
    previousMainSurfaceMinHeightRef.current = mainSurfaceMinHeight
    dragRef.current = null
    setTransientRatio(null)
    setResizing(false)
    suppressGeometryTransition()
  }, [mainSurfaceMinHeight, suppressGeometryTransition])

  const geometry = terminalGeometry(
    transientRatio ?? storedRatio,
    containerHeight,
    mainSurfaceMinHeight
  )
  const terminalPercent = Math.round(geometry.effectiveRatio * 100)
  const mainPercent = 100 - terminalPercent
  const ratioText = t("ratioText", { main: mainPercent, terminal: terminalPercent })
  const drawerHeight = !visible
    ? 0
    : expanded
      ? geometry.allocation
      : Math.min(TERMINAL_HEADER_HEIGHT, geometry.max * geometry.distributableHeight)

  const toggle = () => {
    const next = !expanded
    setExpanded(next)
    void logUserAction(
      "terminal_drawer_toggle",
      next ? "Expanded terminal drawer" : "Collapsed terminal drawer",
      { expanded: next }
    )
  }

  const openSession = () => {
    if (!workspacePath || panes.length >= MAX_PANES) return
    const meta = createTerminalSessionMeta(workspacePath)
    addSession(workspacePath, meta)
    void logUserAction("terminal_new", "Open a new terminal")
  }

  const splitSession = (direction: TerminalSplitDirection) => {
    if (!workspacePath || !activePane || panes.length >= MAX_PANES) return
    const outcome = splitTerminal({
      workspacePath,
      paneId: activePane.paneId,
      sessionId: activePane.sessionId,
    }, direction)
    if (outcome === "completed") {
      void logUserAction(
        direction === "right" ? "terminal_split_right" : "terminal_split_down",
        direction === "right" ? "Split terminal right" : "Split terminal down"
      )
    }
  }

  const closeSession = async () => {
    if (!workspacePath || !activePane || !activeSession) return
    try {
      const outcome = await closeTerminal({
        workspacePath,
        paneId: activePane.paneId,
        sessionId: activeSession.sessionId,
      })
      if (outcome === "completed") {
        void logUserAction("terminal_close", "Close terminal session")
      }
    } catch (error) {
      await showActionError(
        t("contextMenu.cmCloseTerminal", { ns: "menus" }),
        error
      )
    }
  }

  const selectPane = (paneId: string) => {
    if (!workspacePath) return
    setActivePane(workspacePath, paneId)
  }

  const finishDrag = (pointerId: number) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== pointerId) return
    dragRef.current = null
    setTransientRatio(null)
    setResizing(false)
    if (drag.moved && drag.canCommit) setTerminalRatio(workspacePath, drag.ratio)
  }

  const onResizePointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    const container = containerRef.current
    if (!container) return
    const rect = container.getBoundingClientRect()
    const bounds = ratioBounds(rect.height, mainSurfaceMinHeight)
    if (!bounds.canResize) return
    const capturedGeometry = terminalGeometry(
      transientRatio ?? storedRatio,
      rect.height,
      mainSurfaceMinHeight
    )

    event.currentTarget.setPointerCapture(event.pointerId)
    dragRef.current = {
      pointerId: event.pointerId,
      containerBottom: rect.bottom,
      distributableHeight: bounds.distributableHeight,
      initialRatio: capturedGeometry.effectiveRatio,
      ratio: capturedGeometry.effectiveRatio,
      moved: false,
      canCommit: bounds.canResize,
    }
    setTransientRatio(capturedGeometry.effectiveRatio)
    setResizing(true)
    event.preventDefault()
  }

  const onResizePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    const bounds = ratioBounds(
      drag.distributableHeight + ACTIVE_GAP,
      mainSurfaceMinHeight
    )
    const allocation = drag.containerBottom - event.clientY
    const next = clamp(allocation / drag.distributableHeight, bounds.min, bounds.max)
    drag.ratio = next
    drag.moved = next !== drag.initialRatio
    setTransientRatio(next)
    event.preventDefault()
  }

  const onResizePointerUp = (event: PointerEvent<HTMLDivElement>) => {
    finishDrag(event.pointerId)
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
  }

  const onResizeKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (dragRef.current || !geometry.canResize) return
    let direction = 0
    if (event.key === "ArrowUp") direction = 1
    if (event.key === "ArrowDown") direction = -1
    if (direction === 0) return

    event.preventDefault()
    const step = event.shiftKey ? KEYBOARD_LARGE_STEP : KEYBOARD_STEP
    const next = clamp(geometry.effectiveRatio + direction * step, geometry.min, geometry.max)
    if (next !== geometry.effectiveRatio) setTerminalRatio(workspacePath, next)
  }

  return (
    <div
      aria-hidden={!visible}
      inert={!visible}
      data-testid="terminal-drawer"
      className="relative grid min-h-0 shrink-0 transition-[height,grid-template-rows,opacity] duration-[300ms] ease-(--ease-spring)"
      style={{
        height: drawerHeight,
        gridTemplateRows: visible ? "1fr" : "0fr",
        opacity: visible ? 1 : 0,
        // Geometry-driven clamps must apply before paint; otherwise the old
        // height can animate through the active main-surface floor. Two frames
        // later, reveal/collapse transitions are safe to restore.
        transition: resizing || geometryTransitionSuppressed ? "none" : undefined,
      }}
    >
      {/* Mirrors the nav sidebar's reveal (WorkspaceRail toggle): the panel
          springs to full height while fading in as one unit — a card popping
          out — rather than an accordion unfolding from an edge or expanding
          from its centre. opacity:0 while closed also stops the card's drop
          shadow from bleeding a hairline below the editor. No outer
          overflow-hidden so the open card keeps its shadow-lg. */}
      <div
        className="flex h-full min-h-0 flex-col overflow-hidden rounded-(--r-lg) bg-(--term-bg) shadow-(--shadow-lg) ring-1 ring-inset ring-(--term-line)"
      >
        {expanded && (
          <div
            role="separator"
            tabIndex={geometry.canResize ? 0 : -1}
            aria-disabled={geometry.canResize ? undefined : true}
            aria-label={t("resizeAriaLabel")}
            aria-orientation="horizontal"
            aria-valuemin={geometry.canResize ? Math.round(geometry.min * 100) : undefined}
            aria-valuemax={geometry.canResize ? Math.round(geometry.max * 100) : undefined}
            aria-valuenow={geometry.canResize ? terminalPercent : undefined}
            aria-valuetext={geometry.canResize ? ratioText : undefined}
            onPointerDown={onResizePointerDown}
            onPointerMove={onResizePointerMove}
            onPointerUp={onResizePointerUp}
            onPointerCancel={(event) => finishDrag(event.pointerId)}
            onLostPointerCapture={(event) => finishDrag(event.pointerId)}
            onKeyDown={onResizeKeyDown}
            title={geometry.canResize ? t("resizeAriaLabel") : undefined}
            className={`flex h-[6px] shrink-0 touch-none select-none items-center justify-center bg-(--term-bar) focus-visible:bg-(--term-hover) focus-visible:outline-none ${geometry.canResize ? "cursor-row-resize" : "cursor-default"}`}
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
                void closeSession()
              }}
              className={toolButtonClass}
            >
              <X className="size-[13px]" aria-hidden="true" />
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
            height: expanded ? geometry.contentHeight : 0,
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
                    onContextMenu={contextMenuHandler({
                      kind: "terminal",
                      workspacePath,
                      paneId: pane.paneId,
                      sessionId: pane.sessionId,
                    })}
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
      {transientRatio !== null && (
        <div className="pointer-events-none absolute inset-x-0 top-0 h-[6px]">
          <SplitRatioIndicator text={ratioText} />
        </div>
      )}
    </div>
  )
}
