import type { Terminal } from "@xterm/xterm"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { installHerdrDragSelection } from "./herdrDragSelection"
import type { PaneScrollInfo } from "./herdrScrollController"

const installed: Array<{ dispose: () => void }> = []

// 10 cols × 4 rows of 10px cells; HERDR shows rows 50..53 of 104.
function setup(options: { mouseTrackingMode?: string } = {}) {
  const element = document.createElement("div")
  const screen = document.createElement("div")
  screen.className = "xterm-screen"
  element.append(screen)
  document.body.append(element)
  screen.getBoundingClientRect = () => ({ left: 0, top: 0, right: 100, bottom: 40, width: 100, height: 40, x: 0, y: 0, toJSON: () => ({}) })
  let selected = false
  const term = {
    element,
    cols: 10,
    rows: 4,
    modes: { mouseTrackingMode: options.mouseTrackingMode ?? "none" },
    select: vi.fn(() => { selected = true }),
    clearSelection: vi.fn(() => { selected = false }),
    hasSelection: () => selected,
    onSelectionChange: vi.fn(() => ({ dispose: () => undefined })),
  }
  const state: PaneScrollInfo = { offsetFromBottom: 50, maxOffsetFromBottom: 100, viewportRows: 4 }
  const scroll = vi.fn((rows: number) => {
    state.offsetFromBottom = Math.max(0, Math.min(state.maxOffsetFromBottom, state.offsetFromBottom - rows))
  })
  const readText = vi.fn(async () => "selected text")
  const selection = installHerdrDragSelection(term as unknown as Terminal, {
    scrollState: () => state,
    scroll,
    enabled: () => true,
    readText,
  })
  installed.push(selection)
  const mouse = (type: string, x: number, y: number, init: MouseEventInit = {}) => {
    const target = type === "mousedown" ? screen : document
    target.dispatchEvent(new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, ...init }))
  }
  return { term, state, scroll, readText, selection, mouse }
}

describe("HERDR drag selection", () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => {
    installed.splice(0).forEach((selection) => selection.dispose())
    vi.useRealTimers()
    document.body.replaceChildren()
  })

  it("scrolls HERDR while dragging below the frame and reads the whole range", async () => {
    const { term, scroll, readText, selection, mouse } = setup()
    mouse("mousedown", 15, 15) // row 51, col 1
    mouse("mousemove", 15, 55) // 1.5 rows below the frame
    vi.advanceTimersByTime(60)

    expect(scroll).toHaveBeenCalledWith(2)
    // Frame now starts at row 52: the anchor is above it, the cursor on its last row.
    expect(term.select).toHaveBeenLastCalledWith(0, 0, 40)

    mouse("mouseup", 15, 55)
    expect(selection.active()).toBe(true)
    await expect(selection.read()).resolves.toBe("selected text")
    expect(readText).toHaveBeenCalledWith({ row: 51, col: 1 }, { row: 55, col: 9 })
  })

  it("orders the range when dragging above the frame", async () => {
    const { scroll, readText, selection, mouse } = setup()
    mouse("mousedown", 25, 25) // row 52, col 2
    mouse("mousemove", 25, -5)
    vi.advanceTimersByTime(60)
    mouse("mouseup", 25, -5)

    expect(scroll).toHaveBeenCalledWith(-1)
    await selection.read()
    expect(readText).toHaveBeenCalledWith({ row: 49, col: 0 }, { row: 52, col: 2 })
  })

  it("leaves in-frame drags to xterm", () => {
    const { term, scroll, selection, mouse } = setup()
    mouse("mousedown", 15, 15)
    mouse("mousemove", 55, 25)
    vi.advanceTimersByTime(200)
    mouse("mouseup", 55, 25)

    expect(scroll).not.toHaveBeenCalled()
    expect(term.select).not.toHaveBeenCalled()
    expect(selection.active()).toBe(false)
  })

  it("does not take over drags that a mouse-reporting app receives", () => {
    const { scroll, mouse } = setup({ mouseTrackingMode: "drag" })
    mouse("mousedown", 15, 15)
    mouse("mousemove", 15, 55)
    vi.advanceTimersByTime(200)
    expect(scroll).not.toHaveBeenCalled()

    mouse("mouseup", 15, 55)
    mouse("mousedown", 15, 15, { shiftKey: true })
    mouse("mousemove", 15, 55)
    vi.advanceTimersByTime(60)
    expect(scroll).toHaveBeenCalledWith(2)
  })

  it("keeps xterm from re-anchoring once HERDR has scrolled", () => {
    const { mouse } = setup()
    const xtermMove = vi.fn()
    document.addEventListener("mousemove", xtermMove)
    mouse("mousedown", 15, 15)
    mouse("mousemove", 15, 55)
    expect(xtermMove).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(60)
    mouse("mousemove", 15, 60)
    expect(xtermMove).toHaveBeenCalledTimes(1)
    document.removeEventListener("mousemove", xtermMove)
  })

  it("re-maps the highlight on later frames and clears it once off-screen", () => {
    const { term, state, selection, mouse } = setup()
    mouse("mousedown", 15, 15)
    mouse("mousemove", 15, 55)
    vi.advanceTimersByTime(60)
    mouse("mouseup", 15, 55)

    state.offsetFromBottom = 49 // frame rows 51..54
    selection.frame()
    expect(term.select).toHaveBeenLastCalledWith(1, 0, 39)

    state.offsetFromBottom = 0 // frame rows 100..103
    selection.frame()
    expect(term.clearSelection).toHaveBeenCalled()
    expect(selection.active()).toBe(true)

    mouse("mousedown", 15, 15)
    expect(selection.active()).toBe(false)
  })
})
