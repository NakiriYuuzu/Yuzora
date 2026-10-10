import type { IDisposable, Terminal } from "@xterm/xterm"

import type { HerdrMouseAction } from "@/lib/herdrTypes"
import { isTerminalTargetOpenGesture } from "./terminalTarget"
import { terminalWheelCell, type TerminalCell } from "./terminalTransport"

interface HerdrMouseInputOptions {
  /** Page active and visible with a writable connector. */
  enabled: () => boolean
  /** Hover `move` is forwarded only where it is useful (the pane has a detected agent). */
  hoverEnabled?: () => boolean
  send: (action: HerdrMouseAction, cell: TerminalCell, modifiers: number) => void
}

/** crossterm `KeyModifiers` bits HERDR decodes; Shift stays with local selection. */
const CONTROL = 0b10
const ALT = 0b100

const modifiersOf = (event: MouseEvent) => (event.ctrlKey ? CONTROL : 0) | (event.altKey ? ALT : 0)

/**
 * HERDR's JSON connector never tells xterm which mouse mode the child
 * enabled, so xterm never reports clicks itself. Forward left-button presses,
 * drags and releases as `terminal.mouse`: HERDR encodes them for the child's
 * mode and drops them when the child did not ask for mouse reporting, so
 * plain shells keep xterm's local selection. Shift keeps a press local, and
 * the link-open gesture stays with the target opener. Button-less movement is
 * forwarded as `move` on cell changes (one per frame): HERDR only passes it to
 * children that enabled any-motion tracking, such as Claude Code fullscreen.
 */
export function installHerdrMouseInput(term: Terminal, options: HerdrMouseInputOptions): IDisposable {
  const element = term.element
  if (!element) return { dispose: () => undefined }

  /** Last cell sent while the left button is held. */
  let pressed: TerminalCell | null = null

  const screenRect = () => element.querySelector(".xterm-screen")?.getBoundingClientRect() ?? null
  const cellAt = (event: MouseEvent) => {
    const screen = screenRect()
    return screen ? terminalWheelCell(screen, term.cols, term.rows, event.clientX, event.clientY) : undefined
  }

  /** Last hover cell sent, and the pointer position awaiting the next frame. */
  let hovered: TerminalCell | null = null
  let hoverFrame = 0
  let hoverEvent: { x: number; y: number; modifiers: number } | null = null

  const cancelHover = () => {
    if (hoverFrame) cancelAnimationFrame(hoverFrame)
    hoverFrame = 0
    hoverEvent = null
  }

  const flushHover = () => {
    hoverFrame = 0
    const pending = hoverEvent
    hoverEvent = null
    if (!pending || pressed || (term.modes?.mouseTrackingMode ?? "none") !== "none" || !options.enabled() || options.hoverEnabled?.() === false) return
    const screen = screenRect()
    if (
      !screen
      || pending.x < screen.left || pending.x >= screen.right
      || pending.y < screen.top || pending.y >= screen.bottom
    ) return
    const cell = terminalWheelCell(screen, term.cols, term.rows, pending.x, pending.y)
    if (!cell || (hovered && cell.column === hovered.column && cell.row === hovered.row)) return
    hovered = cell
    options.send("move", cell, pending.modifiers)
  }

  const onHover = (event: MouseEvent) => {
    if (pressed || event.buttons !== 0) return
    hoverEvent = { x: event.clientX, y: event.clientY, modifiers: modifiersOf(event) }
    hoverFrame ||= requestAnimationFrame(flushHover)
  }

  // Leaving the pane is not signalled: HERDR has no leave event, so the child
  // keeps its last hover highlight until the pointer returns.
  const onLeave = () => {
    cancelHover()
    hovered = null
  }

  const release = () => {
    pressed = null
    window.removeEventListener("mousemove", onMouseMove, true)
    window.removeEventListener("mouseup", onMouseUp, true)
    window.removeEventListener("blur", endGesture)
  }

  // A release outside the window after it lost focus, outside it without a
  // focus change, or after the terminal went away never arrives: end the
  // gesture at the last cell so the child does not stay mid-drag.
  const endGesture = () => {
    if (!pressed) return
    const cell = pressed
    release()
    options.send("up", cell, 0)
  }

  const onMouseMove = (event: MouseEvent) => {
    // Released outside the window without losing focus: no mouseup arrived.
    if (pressed && (event.buttons & 1) === 0) {
      endGesture()
      return
    }
    const cell = cellAt(event)
    if (!pressed || !cell || (cell.column === pressed.column && cell.row === pressed.row)) return
    pressed = cell
    options.send("drag", cell, modifiersOf(event))
  }

  const onMouseUp = (event: MouseEvent) => {
    if (event.button !== 0 || !pressed) return
    const cell = cellAt(event) ?? pressed
    release()
    options.send("up", cell, modifiersOf(event))
  }

  const onMouseDown = (event: MouseEvent) => {
    if (event.button !== 0 || event.shiftKey || isTerminalTargetOpenGesture(event)) return
    // A child whose modes reach xterm (the official client PTY) is reported by xterm.
    if ((term.modes?.mouseTrackingMode ?? "none") !== "none" || !options.enabled()) return
    const screen = screenRect()
    if (
      !screen
      || event.clientX < screen.left || event.clientX >= screen.right
      || event.clientY < screen.top || event.clientY >= screen.bottom
    ) return
    const cell = terminalWheelCell(screen, term.cols, term.rows, event.clientX, event.clientY)
    if (!cell) return
    release()
    onLeave()
    pressed = cell
    window.addEventListener("mousemove", onMouseMove, true)
    window.addEventListener("mouseup", onMouseUp, true)
    window.addEventListener("blur", endGesture)
    options.send("down", cell, modifiersOf(event))
  }

  element.addEventListener("mousedown", onMouseDown, true)
  element.addEventListener("mousemove", onHover, true)
  element.addEventListener("mouseleave", onLeave)
  return {
    dispose: () => {
      element.removeEventListener("mousedown", onMouseDown, true)
      element.removeEventListener("mousemove", onHover, true)
      element.removeEventListener("mouseleave", onLeave)
      cancelHover()
      endGesture()
      release()
    }
  }
}
