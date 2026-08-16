import { afterEach, describe, expect, it, vi } from "vitest"

import {
  isMacPlatform,
  isWindowsPlatform,
  shortcutLabel,
} from "./platform"

const originalUserAgent = navigator.userAgent

function setUserAgent(userAgent: string) {
  Object.defineProperty(navigator, "userAgent", {
    configurable: true,
    value: userAgent,
  })
}

afterEach(() => {
  setUserAgent(originalUserAgent)
  vi.restoreAllMocks()
})

describe("platform detection", () => {
  it("detects macOS user agents", () => {
    setUserAgent(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"
    )
    expect(isMacPlatform()).toBe(true)
    expect(isWindowsPlatform()).toBe(false)
  })

  it("detects Windows user agents", () => {
    setUserAgent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko)"
    )
    expect(isMacPlatform()).toBe(false)
    expect(isWindowsPlatform()).toBe(true)
  })

  it("treats unknown platforms as non-Mac and non-Windows", () => {
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
    expect(isMacPlatform()).toBe(false)
    expect(isWindowsPlatform()).toBe(false)
  })
})

describe("shortcutLabel", () => {
  it("renders Cmd glyphs on macOS", () => {
    setUserAgent(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15"
    )
    expect(shortcutLabel("mod-k")).toBe("⌘K")
    expect(shortcutLabel("mod-enter")).toBe("⌘↵")
    expect(shortcutLabel("mod-shift-enter")).toBe("⇧⌘↵")
  })

  it("renders Ctrl text on Windows and Linux fallbacks", () => {
    setUserAgent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
    )
    expect(shortcutLabel("mod-k")).toBe("Ctrl+K")
    expect(shortcutLabel("mod-enter")).toBe("Ctrl+Enter")
    expect(shortcutLabel("mod-shift-enter")).toBe("Ctrl+Shift+Enter")

    setUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
    expect(shortcutLabel("mod-k")).toBe("Ctrl+K")
    expect(shortcutLabel("mod-enter")).toBe("Ctrl+Enter")
    expect(shortcutLabel("mod-shift-enter")).toBe("Ctrl+Shift+Enter")
  })
})
