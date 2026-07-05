import { useEffect, useRef, useState } from "react"
import { ChevronDown, ChevronUp, Plus, TerminalSquare } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { useUserActionLog } from "@/features/logs/useUserActionLog"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const MIN_HEIGHT = 140
const MAX_HEIGHT = 480
const DEFAULT_HEIGHT = 228

// The drawer never takes more than this share of the window height, so a
// remembered tall terminal can't swallow the editor when the window shrinks.
const MAX_WINDOW_FRACTION = 0.6

const maxDrawerHeight = () => Math.min(MAX_HEIGHT, Math.round(window.innerHeight * MAX_WINDOW_FRACTION))

interface TerminalDrawerProps {
  visible: boolean
}

/**
 * Terminal drawer — design reference 5.5, a workspace-column sibling of the
 * main area shown for every mode. `visible` is the WorkspaceRail "Toggle
 * terminal" switch — the whole drawer (header included) animates in/out via
 * a grid-rows collapse and is completely hidden (inert) when off. That's a
 * separate concern from `expanded`, which is local: once visible, the
 * drawer's own header/chevron independently controls the resizable content
 * area (starts expanded). No real terminal/process data exists yet, so the
 * header carries no live session chips and the content stays an empty state.
 */
export function TerminalDrawer({ visible }: TerminalDrawerProps) {
  const logAction = useUserActionLog()
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const [expanded, setExpanded] = useState(true)
  const [height, setHeight] = useState(DEFAULT_HEIGHT)
  const [resizing, setResizing] = useState(false)
  const dragRef = useRef<{ startY: number; startHeight: number } | null>(null)

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
    void logAction({
      event: "terminal_drawer_toggle",
      message: next ? "Expanded terminal drawer" : "Collapsed terminal drawer",
      metadata: { expanded: next },
    })
  }

  const logIntent = (event: string, message: string) => {
    void logAction({ event, message })
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
            title="Drag to resize"
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
            <span className="shrink-0 text-[12px] font-semibold text-(--term-fg)">Terminal</span>
            {workspacePath && (
              <span className="min-w-0 flex-1 truncate text-right font-mono text-[10px] text-(--term-fg2)">
                {workspacePath}
              </span>
            )}
          </div>

          <div className="flex shrink-0 items-center gap-[6px]">
            <button
              type="button"
              title="Split right"
              onClick={(event) => {
                event.stopPropagation()
                logIntent("terminal_split_right", "Split terminal right")
              }}
              className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg)"
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
              title="Split down"
              onClick={(event) => {
                event.stopPropagation()
                logIntent("terminal_split_down", "Split terminal down")
              }}
              className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg)"
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
              title="New terminal"
              onClick={(event) => {
                event.stopPropagation()
                logIntent("terminal_new", "Open a new terminal")
              }}
              className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg)"
            >
              <Plus className="size-[13px]" aria-hidden="true" />
            </button>
            <div className="mx-[1px] h-[16px] w-px shrink-0 bg-(--term-line)" />
            <button
              type="button"
              title="Dock terminal into editor"
              onClick={(event) => {
                event.stopPropagation()
                logIntent("terminal_dock_editor", "Dock terminal into editor")
              }}
              className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg)"
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
              aria-label={expanded ? "Collapse terminal" : "Expand terminal"}
              onClick={(event) => {
                event.stopPropagation()
                toggle()
              }}
              className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--term-fg2) transition-colors duration-150 hover:bg-(--term-hover) hover:text-(--term-fg)"
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
          <div className="flex h-full items-center justify-center">
            <EmptyState
              icon={TerminalSquare}
              title="No terminal sessions"
              description="Open a new terminal to run commands here."
              tone="terminal"
            />
          </div>
        </div>
      </div>
    </div>
  )
}
