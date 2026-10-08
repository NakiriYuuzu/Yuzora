import type { IDisposable, Terminal } from "@xterm/xterm"

import type { HerdrMouseAction } from "@/lib/herdrTypes"
import { isTerminalTargetOpenGesture } from "./terminalTarget"
import { terminalWheelCell, type TerminalCell } from "./terminalTransport"

interface HerdrMouseInputOptions {
  /** Page active and visible with a writable connector. */
  enabled: () => boolean
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
 * the link-open gesture stays with the target opener.
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

  const release = () => {
    pressed = null
    window.removeEventListener("mousemove", onMouseMove, true)
    window.removeEventListener("mouseup", onMouseUp, true)
    window.removeEventListener("blur", onBlur)
  }

  // A release outside the window after it lost focus never arrives: end the
  // gesture at the last cell so the child does not stay mid-drag.
  const onBlur = () => {
    if (!pressed) return
    const cell = pressed
    release()
    options.send("up", cell, 0)
  }

  const onMouseMove = (event: MouseEvent) => {
    // Released outside the window without losing focus: no mouseup arrived.
    if (pressed && (event.buttons & 1) === 0) {
      onBlur()
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
    pressed = cell
    window.addEventListener("mousemove", onMouseMove, true)
    window.addEventListener("mouseup", onMouseUp, true)
    window.addEventListener("blur", onBlur)
    options.send("down", cell, modifiersOf(event))
  }

  element.addEventListener("mousedown", onMouseDown, true)
  return {
    dispose: () => {
      element.removeEventListener("mousedown", onMouseDown, true)
      release()
    }
  }
}
