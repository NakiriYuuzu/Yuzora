import type { Terminal } from "@xterm/xterm"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { installHerdrMouseInput } from "./herdrMouseInput"

const installed: Array<{ dispose: () => void }> = []

// 10 cols × 4 rows of 10px cells on macOS, where Cmd-click opens links.
function setup(options: { mouseTrackingMode?: string; enabled?: () => boolean; hoverEnabled?: () => boolean } = {}) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
  const element = document.createElement("div")
  const screen = document.createElement("div")
  screen.className = "xterm-screen"
  element.append(screen)
  document.body.append(element)
  screen.getBoundingClientRect = () => ({ left: 0, top: 0, right: 100, bottom: 40, width: 100, height: 40, x: 0, y: 0, toJSON: () => ({}) })
  const term = { element, cols: 10, rows: 4, modes: { mouseTrackingMode: options.mouseTrackingMode ?? "none" } }
  const send = vi.fn()
  installed.push(installHerdrMouseInput(term as unknown as Terminal, { enabled: options.enabled ?? (() => true), hoverEnabled: options.hoverEnabled, send }))
  const mouse = (type: string, x: number, y: number, init: MouseEventInit = {}) => {
    const target = type === "mousedown" ? screen : document
    target.dispatchEvent(new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, ...init }))
  }
  return { send, mouse, screen, element }
}

describe("HERDR mouse input", () => {
  afterEach(() => {
    installed.splice(0).forEach((input) => input.dispose())
    document.body.replaceChildren()
    vi.restoreAllMocks()
  })

  it("forwards a click as down and up at the pointer cell", () => {
    const { send, mouse } = setup()
    mouse("mousedown", 35, 25)
    mouse("mouseup", 35, 25)
    expect(send.mock.calls).toEqual([
      ["down", { column: 3, row: 2 }, 0],
      ["up", { column: 3, row: 2 }, 0]
    ])
  })

  it("reports drags once per cell and releases outside the screen at the clamped cell", () => {
    const { send, mouse } = setup()
    mouse("mousedown", 5, 5, { ctrlKey: true })
    mouse("mousemove", 7, 7, { buttons: 1, ctrlKey: true })
    mouse("mousemove", 15, 5, { buttons: 1, ctrlKey: true, altKey: true })
    mouse("mouseup", 500, 500)
    mouse("mousemove", 25, 5)
    expect(send.mock.calls).toEqual([
      ["down", { column: 0, row: 0 }, 2],
      ["drag", { column: 1, row: 0 }, 6],
      ["up", { column: 9, row: 3 }, 0]
    ])
  })

  it("ends a drag at its last cell when the window loses focus mid-gesture", () => {
    const { send, mouse } = setup()
    mouse("mousedown", 5, 5)
    mouse("mousemove", 25, 15, { buttons: 1 })
    window.dispatchEvent(new Event("blur"))
    mouse("mousemove", 45, 15)
    mouse("mouseup", 45, 15)
    window.dispatchEvent(new Event("blur"))
    expect(send.mock.calls).toEqual([
      ["down", { column: 0, row: 0 }, 0],
      ["drag", { column: 2, row: 1 }, 0],
      ["up", { column: 2, row: 1 }, 0]
    ])
  })

  it("ends a held gesture at its last cell when the input is disposed", () => {
    const { send, mouse } = setup()
    mouse("mousedown", 5, 5)
    mouse("mousemove", 25, 15, { buttons: 1 })
    installed.splice(0).forEach((input) => input.dispose())
    mouse("mouseup", 25, 15)
    expect(send.mock.calls).toEqual([
      ["down", { column: 0, row: 0 }, 0],
      ["drag", { column: 2, row: 1 }, 0],
      ["up", { column: 2, row: 1 }, 0]
    ])
  })

  it("ends a drag at its last cell when a move arrives with the button already released", () => {
    const { send, mouse } = setup()
    mouse("mousedown", 5, 5)
    mouse("mousemove", 25, 15, { buttons: 1 })
    mouse("mousemove", 45, 25, { buttons: 0 })
    mouse("mousemove", 55, 25, { buttons: 1 })
    mouse("mouseup", 55, 25)
    expect(send.mock.calls).toEqual([
      ["down", { column: 0, row: 0 }, 0],
      ["drag", { column: 2, row: 1 }, 0],
      ["up", { column: 2, row: 1 }, 0]
    ])
  })

  it.each([
    ["Shift-forced selection", { shiftKey: true }, 5],
    ["the link-open gesture", { metaKey: true }, 5],
    ["another button", { button: 2 }, 5],
    ["a press below the screen", {}, 45]
  ])("keeps %s local", (_name, init, y) => {
    const { send, mouse } = setup()
    mouse("mousedown", 5, y, init)
    mouse("mouseup", 5, 5)
    expect(send).not.toHaveBeenCalled()
  })

  describe("hover", () => {
    const hover = (screen: Element, x: number, y: number, init: MouseEventInit = {}) =>
      screen.dispatchEvent(new MouseEvent("mousemove", { clientX: x, clientY: y, bubbles: true, ...init }))
    const frame = () => vi.advanceTimersByTime(20)

    beforeEach(() => vi.useFakeTimers())
    afterEach(() => vi.useRealTimers())

    it("sends one move per frame and only when the cell changes", () => {
      const { send, screen } = setup()
      hover(screen, 5, 5)
      hover(screen, 25, 15)
      hover(screen, 35, 25)
      expect(send).not.toHaveBeenCalled()
      frame()
      hover(screen, 36, 26)
      frame()
      hover(screen, 45, 25, { ctrlKey: true })
      frame()
      expect(send.mock.calls).toEqual([
        ["move", { column: 3, row: 2 }, 0],
        ["move", { column: 4, row: 2 }, 2]
      ])
    })

    it("resends after the pointer leaves, and stays quiet during a press, outside the screen or when disabled", () => {
      const { send, screen, element, mouse } = setup()
      hover(screen, 5, 5)
      frame()
      element.dispatchEvent(new MouseEvent("mouseleave"))
      hover(screen, 5, 5)
      frame()
      mouse("mousedown", 5, 5)
      hover(screen, 25, 5, { buttons: 1 })
      mouse("mouseup", 5, 5)
      frame()
      hover(screen, 5, 45)
      frame()
      expect(send.mock.calls).toEqual([
        ["move", { column: 0, row: 0 }, 0],
        ["move", { column: 0, row: 0 }, 0],
        ["down", { column: 0, row: 0 }, 0],
        ["drag", { column: 2, row: 0 }, 0],
        ["up", { column: 0, row: 0 }, 0]
      ])

      const tracked = setup({ mouseTrackingMode: "vt200" })
      hover(tracked.screen, 5, 5)
      const disabled = setup({ enabled: () => false })
      hover(disabled.screen, 5, 5)
      frame()
      expect(tracked.send).not.toHaveBeenCalled()
      expect(disabled.send).not.toHaveBeenCalled()
    })

    it("forwards hover only while hoverEnabled holds (pane has an agent), never gating clicks", () => {
      let agent = false
      const { send, screen, mouse } = setup({ hoverEnabled: () => agent })
      hover(screen, 5, 5)
      frame()
      expect(send).not.toHaveBeenCalled()
      mouse("mousedown", 5, 5)
      mouse("mouseup", 5, 5)
      expect(send.mock.calls.map((call) => call[0])).toEqual(["down", "up"])
      send.mockClear()
      agent = true
      hover(screen, 25, 5)
      frame()
      expect(send.mock.calls).toEqual([["move", { column: 2, row: 0 }, 0]])
    })

    it("drops a pending hover when the input is disposed", () => {
      const { send, screen } = setup()
      hover(screen, 5, 5)
      installed.splice(0).forEach((input) => input.dispose())
      frame()
      expect(send).not.toHaveBeenCalled()
    })
  })

  it("does nothing when xterm reports mouse itself or the page cannot write", () => {
    const tracked = setup({ mouseTrackingMode: "vt200" })
    tracked.mouse("mousedown", 5, 5)
    tracked.mouse("mouseup", 5, 5)
    expect(tracked.send).not.toHaveBeenCalled()

    const disabled = setup({ enabled: () => false })
    disabled.mouse("mousedown", 5, 5)
    disabled.mouse("mouseup", 5, 5)
    expect(disabled.send).not.toHaveBeenCalled()
  })
})
