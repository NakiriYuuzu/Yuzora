import { afterEach, describe, expect, it, vi } from "vitest"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"
import type { Terminal } from "@xterm/xterm"

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readText: vi.fn(),
  writeText: vi.fn()
}))

import { installTerminalClipboardHandling } from "@/terminal/terminalClipboard"

const readTextMock = vi.mocked(readText)

function terminalStub() {
  const element = document.createElement("div")
  const paste = vi.fn()
  const term = {
    element,
    textarea: null,
    paste,
    hasSelection: vi.fn(() => false),
    getSelection: vi.fn(() => ""),
    attachCustomKeyEventHandler: vi.fn()
  } as unknown as Terminal
  return { element, paste, term }
}

function dispatchPaste(element: HTMLElement, text: string) {
  const event = new Event("paste", {
    bubbles: true,
    cancelable: true
  }) as ClipboardEvent
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (type: string) => type === "text/plain" ? text : ""
    }
  })
  element.dispatchEvent(event)
  return event
}

afterEach(() => {
  readTextMock.mockReset()
  vi.mocked(writeText).mockReset()
  vi.useRealTimers()
  document.body.replaceChildren()
})

describe("terminal image paste and selection copy", () => {
  it.each([
    { key: "v", code: "KeyV", altKey: true },
    { key: "√", code: "KeyV", altKey: true }
  ])("handles the physical Alt/Option V key without typing it: %j", (keys) => {
    const { element, term } = terminalStub()
    const pasteImage = vi.fn()
    let writable = true
    const controller = installTerminalClipboardHandling(term, { pasteImage, canPaste: () => writable })
    controller.flushPendingPaste()
    const key = () => new KeyboardEvent("keydown", { ...keys, bubbles: true, cancelable: true })
    const event = key()
    element.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(true)
    expect(pasteImage).toHaveBeenCalledOnce()
    element.dispatchEvent(new KeyboardEvent("keydown", { ...keys, repeat: true, cancelable: true }))
    writable = false
    element.dispatchEvent(key())
    expect(pasteImage).toHaveBeenCalledOnce()
    expect(readTextMock).not.toHaveBeenCalled()
    controller.dispose()
    element.dispatchEvent(key())
    expect(pasteImage).toHaveBeenCalledOnce()
  })

  it("prefers a pasted PNG over its text fallback and gates image paste in observe mode", () => {
    const { element, term, paste } = terminalStub()
    const pasteImage = vi.fn()
    let writable = true
    const controller = installTerminalClipboardHandling(term, { pasteImage, canPaste: () => writable })
    const png = new File(["fixture"], "image.png", { type: "image/png" })
    const event = () => {
      const e = new Event("paste", { cancelable: true })
      Object.defineProperty(e, "clipboardData", { value: {
        items: [{ type: "image/png", getAsFile: () => png }], getData: () => "fallback text"
      } })
      return e
    }
    element.dispatchEvent(event())
    expect(pasteImage).toHaveBeenCalledExactlyOnceWith(png)
    writable = false
    element.dispatchEvent(event())
    expect(pasteImage).toHaveBeenCalledOnce()
    expect(paste).not.toHaveBeenCalled()
    controller.dispose()
  })

  it("copies once after selection settles, supports release outside the terminal and a live off switch", async () => {
    vi.useFakeTimers()
    const { element, term } = terminalStub()
    vi.mocked(writeText).mockResolvedValue(undefined)
    let enabled = true
    const controller = installTerminalClipboardHandling(term, { copyOnSelect: () => enabled })
    controller.flushPendingPaste()
    const drag = () => {
      element.dispatchEvent(new MouseEvent("mousedown", { button: 0 }))
      element.dispatchEvent(new MouseEvent("mousemove"))
      expect(writeText).not.toHaveBeenCalled()
      window.dispatchEvent(new MouseEvent("mouseup"))
    }
    drag()
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue("selected text")
    await vi.runAllTimersAsync()
    expect(writeText).toHaveBeenCalledExactlyOnceWith("selected text")
    expect(term.hasSelection()).toBe(true)
    vi.mocked(writeText).mockClear()
    enabled = false
    drag()
    await vi.runAllTimersAsync()
    expect(writeText).not.toHaveBeenCalled()
    element.dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, cancelable: true }))
    expect(writeText).toHaveBeenCalledExactlyOnceWith("selected text")
    controller.dispose()
  })

  it("cancels a pending copy on disposal", async () => {
    vi.useFakeTimers()
    const { element, term } = terminalStub()
    vi.mocked(term.hasSelection).mockReturnValue(true)
    const controller = installTerminalClipboardHandling(term, { copyOnSelect: () => true })
    element.dispatchEvent(new MouseEvent("mousedown", { button: 0 }))
    window.dispatchEvent(new MouseEvent("mouseup"))
    controller.dispose()
    await vi.runAllTimersAsync()
    expect(writeText).not.toHaveBeenCalled()
  })
})

describe("terminal clipboard paste buffering", () => {
  it("inserts Shift+Enter as a newline through the paste boundary and blocks xterm's submit key", () => {
    const { element, term, paste } = terminalStub()
    const pasteText = vi.fn()
    const xtermKey = vi.fn()
    let writable = true
    const controller = installTerminalClipboardHandling(term, { pasteText, canPaste: () => writable })
    element.addEventListener("keydown", xtermKey)
    const key = () => new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true, cancelable: true })
    const event = key()
    element.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(true)
    expect(pasteText).toHaveBeenCalledExactlyOnceWith("\n")
    expect(paste).not.toHaveBeenCalled()
    expect(xtermKey).not.toHaveBeenCalled()
    expect(readTextMock).not.toHaveBeenCalled()
    writable = false
    element.dispatchEvent(key())
    writable = true
    controller.flushPendingPaste()
    expect(pasteText).toHaveBeenCalledOnce()
    controller.dispose()
  })

  it.each([
    {}, { shiftKey: true, isComposing: true }, { shiftKey: true, keyCode: 229 },
    { shiftKey: true, ctrlKey: true }, { shiftKey: true, altKey: true }, { shiftKey: true, metaKey: true }
  ])("preserves Enter and IME/other modified Enter handling: %j", (modifier) => {
    const { element, term } = terminalStub()
    const pasteText = vi.fn()
    const controller = installTerminalClipboardHandling(term, { pasteText })
    const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...modifier })
    element.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    expect(pasteText).not.toHaveBeenCalled()
    controller.dispose()
  })

  it("buffers paste only during initial connection setup and flushes once writable", () => {
    let canPaste = false
    const { element, paste, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term, {
      canPaste: () => canPaste
    })

    dispatchPaste(element, "setup paste")
    expect(paste).not.toHaveBeenCalled()

    canPaste = true
    controller.flushPendingPaste()

    expect(paste).toHaveBeenCalledOnce()
    expect(paste).toHaveBeenCalledWith("setup paste")
    controller.dispose()
  })

  it("drops setup-buffered paste when the initial connection remains read-only", () => {
    let canPaste = false
    const { element, paste, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term, {
      canPaste: () => canPaste
    })

    dispatchPaste(element, "observer setup paste")
    controller.flushPendingPaste()

    canPaste = true
    controller.flushPendingPaste()

    expect(paste).not.toHaveBeenCalled()
    controller.dispose()
  })

  it("drops read-only paste attempted after initial connection setup", () => {
    let canPaste = false
    const { element, paste, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term, {
      canPaste: () => canPaste
    })

    controller.flushPendingPaste()
    dispatchPaste(element, "observer paste")

    canPaste = true
    controller.flushPendingPaste()

    expect(paste).not.toHaveBeenCalled()
    controller.dispose()
  })

  it("ignores an observer shortcut before reading the clipboard", async () => {
    let canPaste = false
    const { element, paste, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term, {
      canPaste: () => canPaste
    })
    controller.flushPendingPaste()

    element.dispatchEvent(new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true
    }))
    expect(readTextMock).not.toHaveBeenCalled()

    canPaste = true
    await Promise.resolve()

    expect(paste).not.toHaveBeenCalled()
    controller.dispose()
  })

  it("drops a setup shortcut paste when the initial connection settles read-only", async () => {
    let canPaste = false
    let resolveRead: (value: string) => void = () => {}
    readTextMock.mockImplementationOnce(
      () => new Promise<string>((resolve) => {
        resolveRead = resolve
      })
    )
    const { element, paste, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term, {
      canPaste: () => canPaste
    })

    element.dispatchEvent(new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true
    }))
    expect(readTextMock).toHaveBeenCalledOnce()

    controller.flushPendingPaste()
    canPaste = true
    resolveRead("late setup observer paste")
    await Promise.resolve()
    await Promise.resolve()

    expect(paste).not.toHaveBeenCalled()
    controller.dispose()
  })
})
