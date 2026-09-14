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
  vi.restoreAllMocks()
  document.body.replaceChildren()
})

describe("terminal image paste and selection copy", () => {
  it.each([
    ['Windows native', 'Windows NT 10.0', '\r\n'],
    ['Windows with WSL', 'Windows NT 10.0', '\r\n'],
    ['macOS', 'Macintosh', '\n'],
    ['Linux', 'Linux', '\n'],
  ])('writes %s clipboard line endings using the app OS, not the session shell', async (_, userAgent, eol) => {
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(userAgent)
    const { element, term } = terminalStub()
    vi.mocked(writeText).mockResolvedValue(undefined)
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue('  one\r\n  two\rthree')
    const controller = installTerminalClipboardHandling(term, { canPaste: () => false })
    element.dispatchEvent(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, cancelable: true }))
    await Promise.resolve()
    expect(writeText).toHaveBeenCalledExactlyOnceWith(['one', 'two', 'three'].join(eol))
    controller.dispose()
  })

  it('handles keyboard/custom-key/native-copy duplication once and cancels pending auto-copy', async () => {
    vi.useFakeTimers()
    const { element, term } = terminalStub()
    vi.mocked(writeText).mockResolvedValue(undefined)
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue('  once')
    const controller = installTerminalClipboardHandling(term, { copyOnSelect: () => true })
    element.dispatchEvent(new MouseEvent('mousedown', { button: 0 }))
    window.dispatchEvent(new MouseEvent('mouseup'))
    const key = new KeyboardEvent('keydown', { key: 'c', metaKey: true, cancelable: true })
    element.dispatchEvent(key)
    vi.mocked(term.attachCustomKeyEventHandler).mock.calls[0][0](key)
    element.dispatchEvent(new Event('copy', { cancelable: true }))
    await vi.runAllTimersAsync()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('once')
    expect(term.getSelection).toHaveBeenCalledOnce()
    controller.dispose()
  })

  it('tries browser fallback only after plugin failure and reports both failures once', async () => {
    const fallback = vi.fn().mockRejectedValue(new Error('denied'))
    const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: fallback } })
    const { element, term } = terminalStub(), onCopyError = vi.fn()
    vi.mocked(writeText).mockRejectedValue(new Error('plugin denied'))
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue('  content')
    const controller = installTerminalClipboardHandling(term, { onCopyError })
    element.dispatchEvent(new Event('copy', { cancelable: true }))
    await vi.waitFor(() => expect(onCopyError).toHaveBeenCalledOnce())
    expect(fallback).toHaveBeenCalledExactlyOnceWith('content')
    fallback.mockResolvedValue(undefined)
    element.dispatchEvent(new Event('copy', { cancelable: true }))
    await vi.waitFor(() => expect(fallback).toHaveBeenCalledTimes(2))
    expect(onCopyError).toHaveBeenCalledOnce()
    controller.dispose()
    if (original) Object.defineProperty(navigator, 'clipboard', original)
    else Reflect.deleteProperty(navigator, 'clipboard')
  })

  it('preserves Ctrl+C without selection and never clears the clipboard for empty text', async () => {
    const { element, term } = terminalStub()
    const controller = installTerminalClipboardHandling(term)
    const event = new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, cancelable: true })
    element.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue(' \t\n')
    element.dispatchEvent(new Event('copy', { cancelable: true }))
    await Promise.resolve()
    expect(writeText).not.toHaveBeenCalled()
    controller.dispose()
  })
  it("left-aligns prose and removes terminal padding", async () => {
    const { element, term } = terminalStub()
    vi.mocked(writeText).mockResolvedValue(undefined)
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue("\n\n  output\t \n\t\n")
    const controller = installTerminalClipboardHandling(term)
    element.dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, cancelable: true }))
    await Promise.resolve()
    expect(writeText).toHaveBeenCalledExactlyOnceWith("output")
    controller.dispose()
  })

  it("routes native ClipboardEvent through one formatted plugin write", async () => {
    const { element, term } = terminalStub()
    vi.mocked(writeText).mockResolvedValue(undefined)
    vi.mocked(term.hasSelection).mockReturnValue(true)
    vi.mocked(term.getSelection).mockReturnValue("\n  one  \n  two \t\n")
    const setData = vi.fn()
    const event = new Event("copy", { bubbles: true, cancelable: true }) as ClipboardEvent
    Object.defineProperty(event, "clipboardData", { value: { setData } })
    const controller = installTerminalClipboardHandling(term)
    element.dispatchEvent(event)
    await Promise.resolve()
    expect(setData).not.toHaveBeenCalled()
    expect(writeText).toHaveBeenCalledExactlyOnceWith("one  \ntwo")
    controller.dispose()
  })

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
