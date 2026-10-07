import type { IDisposable, Terminal } from "@xterm/xterm"

import type { PaneScrollInfo } from "./herdrScrollController"
import type { PaneTextPoint } from "./herdrScrollIpc"

interface HerdrDragSelectionOptions {
  /** Latest HERDR pane range; null while pane scrolling is unavailable. */
  scrollState: () => PaneScrollInfo | null
  /** Moves HERDR host scrollback; negative rows reveal older output. */
  scroll: (rows: number) => void
  /** Whether a drag may scroll HERDR (page active, normal screen, writable). */
  enabled: () => boolean
  /** Reads the text between two absolute pane points from HERDR. */
  readText: (anchor: PaneTextPoint, cursor: PaneTextPoint) => Promise<string>
}

export interface HerdrDragSelection extends IDisposable {
  /** True while the last selection spans rows HERDR scrolled during the drag. */
  active: () => boolean
  /** Text of the spanning selection, read from HERDR instead of xterm's frame. */
  read: () => Promise<string>
  /** Call after each parsed HERDR frame so the highlight follows its rows. */
  frame: () => void
}

const AUTOSCROLL_INTERVAL_MS = 60
const MAX_AUTOSCROLL_ROWS = 5

const comparePoints = (a: PaneTextPoint, b: PaneTextPoint) => a.row - b.row || a.col - b.col

/**
 * xterm renders only HERDR's current frame (`scrollback: 0`), so its own
 * drag-scroll clamps the selection to the viewport. While a drag leaves the
 * terminal this scrolls HERDR's host scrollback instead, keeps both ends in
 * absolute pane rows, re-maps xterm's highlight onto each new frame, and lets
 * copy read the full range through `pane.selection.read`.
 */
export function installHerdrDragSelection(term: Terminal, options: HerdrDragSelectionOptions): HerdrDragSelection {
  const element = term.element
  if (!element) return { active: () => false, read: () => Promise.resolve(""), frame: () => undefined, dispose: () => undefined }

  let anchor: PaneTextPoint | null = null
  let cursor: PaneTextPoint | null = null
  let dragging = false
  let span: { anchor: PaneTextPoint; cursor: PaneTextPoint } | null = null
  let startTop = 0
  let pointer: { x: number; y: number } | null = null
  let timer: ReturnType<typeof setInterval> | undefined
  let applying = false

  const screenRect = () => element.querySelector(".xterm-screen")?.getBoundingClientRect() ?? null
  const viewportTop = (state: PaneScrollInfo) => state.maxOffsetFromBottom - state.offsetFromBottom

  /** Pointer → absolute pane point, clamped to the visible frame. */
  const pointAt = (x: number, y: number): { point: PaneTextPoint; edge: number; overflow: number } | null => {
    const state = options.scrollState()
    const rect = screenRect()
    if (!state || !rect || rect.width <= 0 || rect.height <= 0 || term.cols <= 0 || term.rows <= 0) return null
    const cellWidth = rect.width / term.cols
    const cellHeight = rect.height / term.rows
    const edge = y < rect.top ? -1 : y >= rect.bottom ? 1 : 0
    const overflow = edge < 0 ? rect.top - y : edge > 0 ? y - rect.bottom : 0
    const row = edge < 0 ? 0 : edge > 0 ? term.rows - 1 : Math.min(term.rows - 1, Math.floor((y - rect.top) / cellHeight))
    const col = edge < 0 ? 0 : edge > 0 ? term.cols - 1
      : Math.max(0, Math.min(term.cols - 1, Math.floor((x - rect.left) / cellWidth)))
    return { point: { row: viewportTop(state) + row, col }, edge, overflow: overflow / cellHeight }
  }

  /** Re-maps the absolute range onto xterm's current frame. */
  const paint = (from: PaneTextPoint, to: PaneTextPoint) => {
    const state = options.scrollState()
    if (!state) return
    const [start, end] = comparePoints(from, to) <= 0 ? [from, to] : [to, from]
    const top = viewportTop(state)
    const bottom = top + term.rows - 1
    applying = true
    try {
      if (end.row < top || start.row > bottom) {
        term.clearSelection()
        return
      }
      const startRow = Math.max(start.row, top)
      const startCol = start.row < top ? 0 : start.col
      const endRow = Math.min(end.row, bottom)
      const endCol = end.row > bottom ? term.cols - 1 : end.col
      const length = (endRow - startRow) * term.cols + endCol - startCol + 1
      if (length > 0) term.select(startCol, startRow - top, length)
    } finally {
      applying = false
    }
  }

  /** True once HERDR scrolled during the current drag; from then on this module
   * owns the highlight. `term.select` clears xterm's selectionEnd, which also
   * parks xterm's own drag-scroll timer. */
  const scrolledDuringDrag = () => {
    const state = options.scrollState()
    return dragging && state !== null && viewportTop(state) !== startTop
  }

  const repaint = () => {
    if (dragging) {
      if (anchor && cursor && scrolledDuringDrag()) paint(anchor, cursor)
    } else if (span) {
      paint(span.anchor, span.cursor)
    }
  }

  const stopAutoscroll = () => {
    if (timer !== undefined) clearInterval(timer)
    timer = undefined
  }

  const tick = () => {
    if (!dragging || !pointer || !options.enabled()) { stopAutoscroll(); return }
    const hit = pointAt(pointer.x, pointer.y)
    if (!hit || hit.edge === 0) { stopAutoscroll(); return }
    const rows = Math.min(MAX_AUTOSCROLL_ROWS, 1 + Math.floor(hit.overflow))
    options.scroll(hit.edge * rows)
    const moved = pointAt(pointer.x, pointer.y)
    if (moved) cursor = moved.point
    repaint()
  }

  const onMouseMove = (event: MouseEvent) => {
    if (!dragging) return
    pointer = { x: event.clientX, y: event.clientY }
    const hit = pointAt(event.clientX, event.clientY)
    if (!hit) return
    cursor = hit.point
    if (hit.edge !== 0 && timer === undefined) timer = setInterval(tick, AUTOSCROLL_INTERVAL_MS)
    if (hit.edge === 0) stopAutoscroll()
    if (scrolledDuringDrag()) {
      // xterm would re-anchor the selection to its stale frame coordinates.
      event.stopPropagation()
      repaint()
    }
  }

  const onMouseUp = () => {
    if (!dragging) return
    dragging = false
    stopAutoscroll()
    window.removeEventListener("mousemove", onMouseMove, true)
    window.removeEventListener("mouseup", onMouseUp, true)
    const state = options.scrollState()
    // Only take over when HERDR scrolled during the drag; otherwise xterm's
    // own selection (and its copy formatting) already covers the range.
    if (anchor && cursor && state && viewportTop(state) !== startTop && comparePoints(anchor, cursor) !== 0) {
      span = { anchor, cursor }
      paint(anchor, cursor)
    }
  }

  const onMouseDown = (event: MouseEvent) => {
    span = null
    if (event.button !== 0 || !options.enabled()) return
    // Mouse-reporting applications receive the drag unless Shift forces selection.
    if ((term.modes?.mouseTrackingMode ?? "none") !== "none" && !event.shiftKey) return
    const state = options.scrollState()
    const hit = pointAt(event.clientX, event.clientY)
    if (!state || !hit || hit.edge !== 0) return
    anchor = hit.point
    cursor = hit.point
    startTop = viewportTop(state)
    pointer = { x: event.clientX, y: event.clientY }
    dragging = true
    window.addEventListener("mousemove", onMouseMove, true)
    window.addEventListener("mouseup", onMouseUp, true)
  }

  element.addEventListener("mousedown", onMouseDown, true)
  // Some embedded/test adapters expose only part of xterm's event surface.
  const selectionDisposable = term.onSelectionChange?.(() => {
    if (!applying && !dragging && span && !term.hasSelection()) span = null
  })

  return {
    active: () => span !== null,
    frame: repaint,
    read: () => {
      if (!span) return Promise.resolve("")
      const [start, end] = comparePoints(span.anchor, span.cursor) <= 0 ? [span.anchor, span.cursor] : [span.cursor, span.anchor]
      return options.readText(start, end)
    },
    dispose: () => {
      stopAutoscroll()
      element.removeEventListener("mousedown", onMouseDown, true)
      window.removeEventListener("mousemove", onMouseMove, true)
      window.removeEventListener("mouseup", onMouseUp, true)
      selectionDisposable?.dispose()
      span = null
    }
  }
}
