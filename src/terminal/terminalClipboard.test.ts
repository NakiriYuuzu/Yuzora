import { afterEach, describe, expect, it, vi } from "vitest"
import { readText } from "@tauri-apps/plugin-clipboard-manager"
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
  document.body.replaceChildren()
})

describe("terminal clipboard paste buffering", () => {
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
