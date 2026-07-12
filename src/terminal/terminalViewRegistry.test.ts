import { describe, expect, it, vi } from "vitest"

import {
  getTerminalView,
  registerTerminalView,
  type TerminalViewHandle,
} from "./terminalViewRegistry"

function handle(): TerminalViewHandle {
  return {
    hasSelection: vi.fn(() => false),
    getSelection: vi.fn(() => ""),
    paste: vi.fn(),
    clear: vi.fn(),
  }
}

describe("terminalViewRegistry", () => {
  it("registers the minimal view handle and unregisters it", () => {
    const view = handle()
    const unregister = registerTerminalView("session-1", view)

    expect(getTerminalView("session-1")).toBe(view)
    unregister()
    expect(getTerminalView("session-1")).toBeUndefined()
  })

  it("does not let an older cleanup unregister a replacement handle", () => {
    const first = handle()
    const second = handle()
    const unregisterFirst = registerTerminalView("session-1", first)
    const unregisterSecond = registerTerminalView("session-1", second)

    unregisterFirst()
    expect(getTerminalView("session-1")).toBe(second)

    unregisterSecond()
    expect(getTerminalView("session-1")).toBeUndefined()
  })
})
