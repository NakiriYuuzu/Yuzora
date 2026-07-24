import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
} from "react"
import { ChevronDown, ChevronUp, CircleAlert, List, Plus, TerminalSquare, X } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { SplitRatioIndicator } from "@/app/workbench/SplitRatioIndicator"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
import type { TerminalProfile } from "@/lib/types"
import { loadTerminalSettings } from "@/app/workbench/settingsStorage"
import { contextMenuHandler } from "@/state/contextMenuStore"
import {
  MAX_TERMINAL_PANE_SPLIT_RATIO,
  MAX_VISIBLE_TERMINAL_PANES,
  MIN_TERMINAL_PANE_SPLIT_RATIO,
  terminalDisplayTitle,
  useTerminalStore,
} from "@/state/terminalStore"
import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { TerminalSession } from "@/terminal/TerminalSession"
import {
  closeTerminal,
  createTerminalSessionMeta,
  splitTerminal,
  type TerminalSessionMetaWithArgs,
} from "@/terminal/terminalCommands"
import {
  availableTerminalProfiles,
  terminalProfileDisplayName,
} from "@/terminal/terminalProfiles"
import { useTerminalProfiles } from "@/terminal/useTerminalProfiles"

const ACTIVE_GAP = 10
const TERMINAL_CONTENT_MIN_HEIGHT = 140
const TERMINAL_HANDLE_HEIGHT = 6
const TERMINAL_HEADER_HEIGHT = 38
const TERMINAL_FIXED_CHROME_HEIGHT = TERMINAL_HANDLE_HEIGHT + TERMINAL_HEADER_HEIGHT
const KEYBOARD_STEP = 0.02
const KEYBOARD_LARGE_STEP = 0.1
const EMPTY_TERMINAL_TAB_IDS: string[] = []

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

interface PaneDragState {
  pointerId: number
  workspace: string
  paneKey: string
  left: number
  width: number
  ratio: number
}

interface TransientPaneRatio {
  workspace: string
  paneKey: string
  ratio: number
}

interface TerminalRenameInputProps {
  initialTitle: string
  ariaLabel: string
  onSave: (title: string) => void
  onCancel: () => void
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function TerminalRenameInput({
  initialTitle,
  ariaLabel,
  onSave,
  onCancel,
}: TerminalRenameInputProps) {
  const [draft, setDraft] = useState(initialTitle)
  const cancelledRef = useRef(false)

  return (
    <input
      autoFocus
      aria-label={ariaLabel}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onFocus={(event) => event.currentTarget.select()}
      onClick={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onBlur={() => {
        if (cancelledRef.current) return
        onSave(draft)
      }}
      onKeyDown={(event) => {
        event.stopPropagation()
        if (event.key === "Enter") event.currentTarget.blur()
        if (event.key === "Escape") {
          event.preventDefault()
          cancelledRef.current = true
          onCancel()
        }
      }}
      className="min-w-0 flex-1 rounded-[3px] border border-(--term-blue) bg-(--term-bg) px-[4px] py-[1px] text-[11px] text-(--term-fg) outline-none"
    />
  )
}

function ratioBounds(
  containerHeight: number,
  mainSurfaceMinHeight: number,
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
  mainSurfaceMinHeight: number,
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
  const layouts = useTerminalStore((s) => s.layouts)
  const layout = workspacePath ? layouts[workspacePath] : undefined
  const addSession = useTerminalStore((s) => s.addSession)
  const removeSession = useTerminalStore((s) => s.removeSession)
  const selectTab = useTerminalStore((s) => s.selectTab)
  const reorderTab = useTerminalStore((s) => s.reorderTab)
  const setActivePane = useTerminalStore((s) => s.setActivePane)
  const setSplitRatio = useTerminalStore((s) => s.setSplitRatio)
  const beginRename = useTerminalStore((s) => s.beginRename)
  const finishRename = useTerminalStore((s) => s.finishRename)
  const setManualTitle = useTerminalStore((s) => s.setManualTitle)
  const setShellTitle = useTerminalStore((s) => s.setShellTitle)
  const setLaunchStatus = useTerminalStore((s) => s.setLaunchStatus)
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
  const [transientPaneRatio, setTransientPaneRatio] = useState<TransientPaneRatio | null>(null)
  const [resizing, setResizing] = useState(false)
  const [geometryTransitionSuppressed, setGeometryTransitionSuppressed] = useState(false)
  const discoveredProfiles = useTerminalProfiles()
  const dragRef = useRef<DragState | null>(null)
  const paneDragRef = useRef<PaneDragState | null>(null)
  const paneGridRef = useRef<HTMLDivElement | null>(null)
  const draggedTabRef = useRef<string | null>(null)
  const tabRefs = useRef(new Map<string, HTMLDivElement>())
  const measuredContainerHeightRef = useRef(0)
  const transitionResetFrameRef = useRef<number | null>(null)
  const previousMainSurfaceMinHeightRef = useRef(mainSurfaceMinHeight)
  const panes = layout?.panes ?? []
  const paneKey = panes.map((pane) => pane.paneId).join(":")
  const tabIds = layout?.tabIds ?? EMPTY_TERMINAL_TAB_IDS
  const workspaceLayouts = Object.entries(layouts).filter(
    ([, candidate]) => candidate.tabIds.length > 0,
  )
  const activePaneId = layout?.activePaneId ?? null
  const activePane = panes.find((pane) => pane.paneId === activePaneId)
  const activeSession = activePane ? sessions[activePane.sessionId] : undefined
  const focusedSessionId = activeSession?.sessionId ?? null
  const canCreateSession = Boolean(workspacePath)
  const canSplit = Boolean(workspacePath && activePane) && panes.length < MAX_VISIBLE_TERMINAL_PANES
  const paneRatio =
    transientPaneRatio?.workspace === workspacePath && transientPaneRatio.paneKey === paneKey
      ? transientPaneRatio.ratio
      : (layout?.splitRatio ?? 0.5)

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

  useLayoutEffect(
    () => () => {
      if (transitionResetFrameRef.current !== null) {
        cancelAnimationFrame(transitionResetFrameRef.current)
      }
    },
    [],
  )

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

  useLayoutEffect(() => {
    const targetId = layout?.renamingSessionId ?? focusedSessionId
    if (!targetId) return
    tabRefs.current.get(targetId)?.scrollIntoView?.({
      behavior: "smooth",
      block: "nearest",
      inline: "nearest",
    })
  }, [focusedSessionId, layout?.renamingSessionId, tabIds])

  const geometry = terminalGeometry(
    transientRatio ?? storedRatio,
    containerHeight,
    mainSurfaceMinHeight,
  )
  const terminalPercent = Math.round(geometry.effectiveRatio * 100)
  const mainPercent = 100 - terminalPercent
  const ratioText = t("ratioText", {
    main: mainPercent,
    terminal: terminalPercent,
  })
  const leftPanePercent = Math.round(paneRatio * 100)
  const rightPanePercent = 100 - leftPanePercent
  const paneRatioText = t("terminalDrawer.paneRatioText", {
    ns: "menus",
    left: leftPanePercent,
    right: rightPanePercent,
  })
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
      { expanded: next },
    )
  }

  const expandForTerminalAction = () => {
    if (!expanded) setExpanded(true)
  }

  const openSession = (profile?: TerminalProfile) => {
    if (!workspacePath) return
    expandForTerminalAction()
    const meta = createTerminalSessionMeta(workspacePath, profile)
    addSession(workspacePath, meta)
    void logUserAction("terminal_new", "Open a new terminal", {
      profileId: profile?.id ?? terminalSettings.defaultProfile.id,
    })
  }

  const splitSession = () => {
    if (!workspacePath || !activePane || panes.length >= MAX_VISIBLE_TERMINAL_PANES) return
    expandForTerminalAction()
    const outcome = splitTerminal({
      workspacePath,
      paneId: activePane.paneId,
      sessionId: activePane.sessionId,
    })
    if (outcome === "completed") {
      void logUserAction("terminal_split_right", "Split terminal right")
    }
  }

  const closeSession = async (sessionWorkspace: string, sessionId: string) => {
    try {
      const outcome = await closeTerminal({
        workspacePath: sessionWorkspace,
        sessionId,
      })
      if (outcome === "completed") {
        void logUserAction("terminal_close", "Close terminal session")
      }
    } catch (error) {
      await showActionError(t("contextMenu.cmCloseTerminal", { ns: "menus" }), error)
    }
  }

  const terminalSettings = loadTerminalSettings()
  const selectableProfiles = availableTerminalProfiles(
    discoveredProfiles,
    terminalSettings.defaultProfile,
    terminalSettings.customProfile,
  )

  const selectTerminalTab = (sessionId: string) => {
    if (!workspacePath) return
    expandForTerminalAction()
    selectTab(workspacePath, sessionId)
  }

  const selectPane = (paneWorkspace: string, paneId: string) => {
    setActivePane(paneWorkspace, paneId)
  }

  const onTabListKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      event.target instanceof HTMLInputElement ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey
    ) {
      return
    }
    if (tabIds.length === 0) return

    const currentIndex = Math.max(0, tabIds.indexOf(focusedSessionId ?? ""))
    let nextIndex: number
    if (event.key === "ArrowLeft") nextIndex = Math.max(0, currentIndex - 1)
    else if (event.key === "ArrowRight") nextIndex = Math.min(tabIds.length - 1, currentIndex + 1)
    else if (event.key === "Home") nextIndex = 0
    else if (event.key === "End") nextIndex = tabIds.length - 1
    else return

    event.preventDefault()
    const nextId = tabIds[nextIndex]
    selectTerminalTab(nextId)
    requestAnimationFrame(() => tabRefs.current.get(nextId)?.focus())
  }

  const onTabDragStart = (event: DragEvent<HTMLDivElement>, sessionId: string) => {
    if (layout?.renamingSessionId) {
      event.preventDefault()
      return
    }
    draggedTabRef.current = sessionId
    event.dataTransfer.effectAllowed = "move"
    event.dataTransfer.setData("text/plain", sessionId)
  }

  const onTabDrop = (event: DragEvent<HTMLDivElement>, destinationIndex: number) => {
    event.preventDefault()
    if (!workspacePath || layout?.renamingSessionId) return
    const sessionId = draggedTabRef.current || event.dataTransfer.getData("text/plain")
    if (sessionId) reorderTab(workspacePath, sessionId, destinationIndex)
    draggedTabRef.current = null
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
      mainSurfaceMinHeight,
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
    const bounds = ratioBounds(drag.distributableHeight + ACTIVE_GAP, mainSurfaceMinHeight)
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

  const finishPaneResize = (pointerId: number) => {
    const drag = paneDragRef.current
    if (!drag || drag.pointerId !== pointerId) return
    paneDragRef.current = null
    setTransientPaneRatio(null)
    setSplitRatio(drag.workspace, drag.ratio)
  }

  const onPaneResizePointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (
      event.button !== 0 ||
      !workspacePath ||
      panes.length !== MAX_VISIBLE_TERMINAL_PANES
    ) {
      return
    }
    const grid = paneGridRef.current
    if (!grid) return
    const rect = grid.getBoundingClientRect()
    if (rect.width <= 0) return
    event.currentTarget.setPointerCapture(event.pointerId)
    paneDragRef.current = {
      pointerId: event.pointerId,
      workspace: workspacePath,
      paneKey,
      left: rect.left,
      width: rect.width,
      ratio: paneRatio,
    }
    setTransientPaneRatio({ workspace: workspacePath, paneKey, ratio: paneRatio })
    event.preventDefault()
  }

  const onPaneResizePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = paneDragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    const next = clamp(
      (event.clientX - drag.left) / drag.width,
      MIN_TERMINAL_PANE_SPLIT_RATIO,
      MAX_TERMINAL_PANE_SPLIT_RATIO,
    )
    drag.ratio = next
    setTransientPaneRatio({
      workspace: drag.workspace,
      paneKey: drag.paneKey,
      ratio: next,
    })
    event.preventDefault()
  }

  const onPaneResizePointerUp = (event: PointerEvent<HTMLDivElement>) => {
    finishPaneResize(event.pointerId)
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
  }

  const onPaneResizeKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!workspacePath || panes.length !== MAX_VISIBLE_TERMINAL_PANES) return
    let direction = 0
    if (event.key === "ArrowLeft") direction = -1
    if (event.key === "ArrowRight") direction = 1
    if (direction === 0) return
    event.preventDefault()
    const step = event.shiftKey ? KEYBOARD_LARGE_STEP : KEYBOARD_STEP
    setSplitRatio(workspacePath, paneRatio + direction * step)
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
      <div className="flex h-full min-h-0 flex-col overflow-hidden rounded-(--r-lg) bg-(--term-bg) shadow-(--shadow-lg) ring-1 ring-inset ring-(--term-line)">
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

        <div
          data-testid="terminal-header"
          className="flex h-[38px] shrink-0 items-center gap-[7px] bg-(--term-bar) px-[9px]"
        >
          <span
            role="img"
            aria-label={t("terminalDrawer.iconAriaLabel", { ns: "menus" })}
            className="flex size-[20px] shrink-0 items-center justify-center text-(--term-fg2)"
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
              <path d="m5 8 4 4-4 4M13 16h6" />
            </svg>
          </span>

          <div
            role="tablist"
            aria-label={t("terminalDrawer.tabListAriaLabel", { ns: "menus" })}
            data-testid="terminal-tab-list"
            onKeyDown={onTabListKeyDown}
            className="flex min-w-0 flex-1 items-stretch gap-[3px] overflow-x-auto py-[3px] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
          >
            {tabIds.map((sessionId, index) => {
              const session = sessions[sessionId]
              if (!session) return null
              const displayTitle = terminalDisplayTitle(session)
              const visiblePaneIndex = panes.findIndex((pane) => pane.sessionId === sessionId)
              const focused = sessionId === focusedSessionId
              const renaming = sessionId === layout?.renamingSessionId
              const closeLabel = t("terminalDrawer.closeTabAriaLabel", {
                ns: "menus",
                title: displayTitle,
              })

              return (
                <div
                  key={sessionId}
                  ref={(element) => {
                    if (element) tabRefs.current.set(sessionId, element)
                    else tabRefs.current.delete(sessionId)
                  }}
                  role="tab"
                  tabIndex={focused || (!focusedSessionId && index === 0) ? 0 : -1}
                  aria-selected={focused}
                  aria-label={displayTitle}
                  data-session-id={sessionId}
                  data-visible-pane={
                    visiblePaneIndex === 0 ? "left" : visiblePaneIndex === 1 ? "right" : "hidden"
                  }
                  data-focused={String(focused)}
                  draggable={!renaming}
                  onClick={() => selectTerminalTab(sessionId)}
                  onDoubleClick={() => beginRename(workspacePath!, sessionId)}
                  onContextMenu={
                    renaming || !workspacePath
                      ? (event) => event.preventDefault()
                      : contextMenuHandler({
                          kind: "terminalTab",
                          workspacePath,
                          sessionId,
                        })
                  }
                  onDragStart={(event) => onTabDragStart(event, sessionId)}
                  onDragEnd={() => {
                    draggedTabRef.current = null
                  }}
                  onDragOver={(event) => {
                    if (!layout?.renamingSessionId) event.preventDefault()
                  }}
                  onDrop={(event) => onTabDrop(event, index)}
                  className={`group/tab flex h-[30px] min-w-[112px] max-w-[220px] shrink items-center gap-[5px] rounded-[6px] border px-[8px] text-[11px] outline-none transition-colors ${
                    focused
                      ? "border-(--term-blue) bg-(--term-bg) text-(--term-fg)"
                      : visiblePaneIndex >= 0
                        ? "border-(--term-line) bg-(--term-hover) text-(--term-fg)"
                        : "border-transparent text-(--term-fg2) hover:bg-(--term-hover) hover:text-(--term-fg)"
                  } focus-visible:ring-1 focus-visible:ring-(--term-blue)`}
                >
                  {renaming ? (
                    <TerminalRenameInput
                      initialTitle={displayTitle}
                      ariaLabel={t("terminalDrawer.renameInputAriaLabel", {
                        ns: "menus",
                        title: displayTitle,
                      })}
                      onSave={(title) => {
                        setManualTitle(sessionId, title)
                        finishRename(workspacePath!, sessionId)
                      }}
                      onCancel={() => finishRename(workspacePath!, sessionId)}
                    />
                  ) : (
                    <>
                      {session.launchStatus === "failed" && (
                        <CircleAlert
                          className="size-[12px] shrink-0 text-destructive"
                          aria-label={t("terminalDrawer.spawnFailedTitle", {
                            ns: "menus",
                          })}
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate" title={displayTitle}>
                        {displayTitle}
                      </span>
                      <button
                        type="button"
                        title={closeLabel}
                        aria-label={closeLabel}
                        draggable={false}
                        onClick={(event) => {
                          event.stopPropagation()
                          void closeSession(workspacePath!, sessionId)
                        }}
                        onDoubleClick={(event) => event.stopPropagation()}
                        onDragStart={(event) => event.preventDefault()}
                        className={`flex size-[16px] shrink-0 items-center justify-center rounded-[4px] text-(--term-fg2) transition-opacity hover:bg-(--term-hover) hover:text-(--term-fg) focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-(--term-blue) ${
                          focused
                            ? "opacity-100"
                            : "opacity-0 group-hover/tab:opacity-100 group-focus-within/tab:opacity-100"
                        }`}
                      >
                        <X className="size-[11px]" aria-hidden="true" />
                      </button>
                    </>
                  )}
                </div>
              )
            })}
          </div>

          <div className="flex shrink-0 items-center gap-[5px]">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  title={t("terminalDrawer.allTerminalsTitle", { ns: "menus" })}
                  aria-label={t("terminalDrawer.allTerminalsTitle", {
                    ns: "menus",
                  })}
                  disabled={tabIds.length === 0}
                  className={toolButtonClass}
                >
                  <List className="size-[13px]" aria-hidden="true" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-[260px]">
                {tabIds.map((sessionId) => {
                  const session = sessions[sessionId]
                  if (!session) return null
                  const visiblePaneIndex = panes.findIndex((pane) => pane.sessionId === sessionId)
                  const focused = sessionId === focusedSessionId
                  const paneLabel =
                    visiblePaneIndex === 0
                      ? t("terminalDrawer.leftPane", { ns: "menus" })
                      : visiblePaneIndex === 1
                        ? t("terminalDrawer.rightPane", { ns: "menus" })
                        : null
                  const focusedLabel = focused
                    ? t("terminalDrawer.focusedPane", { ns: "menus" })
                    : null
                  const statusLabel = [paneLabel, focusedLabel].filter(Boolean).join(" · ")
                  const displayTitle = terminalDisplayTitle(session)
                  return (
                    <DropdownMenuItem
                      key={sessionId}
                      aria-label={statusLabel ? `${displayTitle} · ${statusLabel}` : displayTitle}
                      onSelect={() => selectTerminalTab(sessionId)}
                      className="min-w-0"
                    >
                      {session.launchStatus === "failed" && (
                        <CircleAlert className="size-[12px] text-destructive" aria-hidden="true" />
                      )}
                      <span className="min-w-0 flex-1 truncate">{displayTitle}</span>
                      {statusLabel && (
                        <span className="shrink-0 text-[10px] text-muted-foreground">
                          {statusLabel}
                        </span>
                      )}
                    </DropdownMenuItem>
                  )
                })}
              </DropdownMenuContent>
            </DropdownMenu>
            <button
              type="button"
              title={t(
                panes.length >= MAX_VISIBLE_TERMINAL_PANES
                  ? "terminalDrawer.splitLimitTitle"
                  : "terminalDrawer.splitRightTitle",
                { ns: "menus" },
              )}
              disabled={!canSplit}
              onClick={splitSession}
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
            <div
              role="group"
              aria-label={t("terminalDrawer.newTerminalTitle", { ns: "menus" })}
              className="flex items-center rounded-[7px]"
            >
              <button
                type="button"
                title={t("terminalDrawer.newTerminalTitle", { ns: "menus" })}
                disabled={!canCreateSession}
                onClick={() => openSession()}
                className={toolButtonClass}
              >
                <Plus className="size-[13px]" aria-hidden="true" />
              </button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    title={t("terminalDrawer.selectProfileTitle", { ns: "menus" })}
                    aria-label={t("terminalDrawer.selectProfileTitle", { ns: "menus" })}
                    disabled={!canCreateSession}
                    className={toolButtonClass}
                  >
                    <ChevronDown className="size-[13px]" aria-hidden="true" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-[240px]">
                  {selectableProfiles.map((profile) => (
                    <DropdownMenuItem key={profile.id} onSelect={() => openSession(profile)}>
                      <span className="min-w-0 flex-1 truncate">
                        {terminalProfileDisplayName(profile)}
                      </span>
                      {profile.id === terminalSettings.defaultProfile.id && (
                        <span className="text-[10px] text-muted-foreground">
                          {t("terminalDrawer.defaultProfile", { ns: "menus" })}
                        </span>
                      )}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <button
              type="button"
              aria-expanded={expanded}
              aria-label={
                expanded
                  ? t("terminalDrawer.collapseAriaLabel", { ns: "menus" })
                  : t("terminalDrawer.expandAriaLabel", { ns: "menus" })
              }
              onClick={toggle}
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
          onContextMenu={(event) => {
            event.preventDefault()
            event.stopPropagation()
          }}
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
          <div className="relative h-full min-h-0">
            {tabIds.length === 0 && (
              <div className="flex h-full items-center justify-center">
                <EmptyState
                  icon={TerminalSquare}
                  title={t("noSessions")}
                  description={t("emptyDescription")}
                  tone="terminal"
                />
              </div>
            )}
            {workspaceLayouts.map(([paneWorkspace, paneLayout]) => {
              const isCurrentWorkspace = paneWorkspace === workspacePath
              const effectivePaneRatio = isCurrentWorkspace ? paneRatio : paneLayout.splitRatio
              const splitVisible = paneLayout.panes.length === MAX_VISIBLE_TERMINAL_PANES
              return (
                <div
                  key={paneWorkspace}
                  ref={isCurrentWorkspace ? paneGridRef : undefined}
                  hidden={!isCurrentWorkspace}
                  aria-hidden={!isCurrentWorkspace}
                  inert={!isCurrentWorkspace}
                  className={`${isCurrentWorkspace ? "grid" : "hidden"} h-full min-h-0 overflow-hidden`}
                  style={{
                    gridTemplateColumns: splitVisible
                      ? `${effectivePaneRatio}fr 4px ${1 - effectivePaneRatio}fr`
                      : "minmax(0, 1fr)",
                    gridTemplateRows: "minmax(0, 1fr)",
                  }}
                  data-testid={isCurrentWorkspace ? "terminal-pane-grid" : undefined}
                >
                  {paneLayout.tabIds.map((sessionId) => {
                    const session = sessions[sessionId]
                    if (!session) return null
                    const visiblePaneIndex = paneLayout.panes.findIndex(
                      (pane) => pane.sessionId === sessionId,
                    )
                    const pane = visiblePaneIndex >= 0 ? paneLayout.panes[visiblePaneIndex] : null
                    const sessionVisible = Boolean(
                      visible && expanded && isCurrentWorkspace && pane,
                    )
                    const active = Boolean(
                      sessionVisible && pane?.paneId === paneLayout.activePaneId,
                    )
                    const shellArgs = (session as TerminalSessionMetaWithArgs).shellArgs

                    return (
                      <div
                        key={sessionId}
                        data-testid={pane ? `terminal-pane-${pane.paneId}` : undefined}
                        data-session-id={sessionId}
                        onClick={pane ? () => selectPane(paneWorkspace, pane.paneId) : undefined}
                        className={`min-h-0 min-w-0 overflow-hidden ${
                          active ? "ring-1 ring-inset ring-(--term-blue)" : ""
                        }`}
                        style={{
                          display: pane ? undefined : "none",
                          gridColumn: visiblePaneIndex === 1 ? 3 : 1,
                          gridRow: 1,
                        }}
                      >
                        <TerminalSession
                          workspace={paneWorkspace}
                          sessionId={session.sessionId}
                          shell={session.shell || null}
                          shellArgs={shellArgs}
                          cwdStrategy={session.cwdStrategy}
                          imeAnchorMode={session.imeAnchorMode}
                          active={active}
                          visible={sessionVisible}
                          onExit={() => removeSession(paneWorkspace, session.sessionId)}
                          onTitleChange={(title) => setShellTitle(session.sessionId, title)}
                          onReady={() => setLaunchStatus(session.sessionId, "running")}
                          onOpenError={() => setLaunchStatus(session.sessionId, "failed")}
                        />
                      </div>
                    )
                  })}
                  {isCurrentWorkspace && splitVisible && (
                    <div
                      role="separator"
                      tabIndex={0}
                      aria-label={t("terminalDrawer.paneResizeAriaLabel", {
                        ns: "menus",
                      })}
                      aria-orientation="vertical"
                      aria-valuemin={MIN_TERMINAL_PANE_SPLIT_RATIO * 100}
                      aria-valuemax={MAX_TERMINAL_PANE_SPLIT_RATIO * 100}
                      aria-valuenow={leftPanePercent}
                      aria-valuetext={paneRatioText}
                      title={t("terminalDrawer.paneResizeAriaLabel", {
                        ns: "menus",
                      })}
                      data-testid="terminal-pane-divider"
                      onPointerDown={onPaneResizePointerDown}
                      onPointerMove={onPaneResizePointerMove}
                      onPointerUp={onPaneResizePointerUp}
                      onPointerCancel={(event) => finishPaneResize(event.pointerId)}
                      onLostPointerCapture={(event) => finishPaneResize(event.pointerId)}
                      onKeyDown={onPaneResizeKeyDown}
                      className="z-10 col-start-2 row-start-1 h-full w-[4px] cursor-col-resize touch-none bg-(--term-line) outline-none transition-colors hover:bg-(--term-blue) focus-visible:bg-(--term-blue)"
                    />
                  )}
                </div>
              )
            })}
          </div>
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
