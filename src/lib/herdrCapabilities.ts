import type { HerdrCapabilities } from "./herdrTypes"

/**
 * Keep HERDR capability decisions in one adapter. The server protocol and
 * schema are runtime inputs; version numbers alone do not prove a method is
 * callable.
 */
export function hasHerdrMethod(
  capabilities: HerdrCapabilities | null | undefined,
  method: string
): boolean {
  return capabilities?.api.methods.includes(method) ?? false
}

/** Protocol 22 introduced the pane-owned scroll position API. */
export function supportsHerdrPaneScroll(
  capabilities: HerdrCapabilities | null | undefined
): boolean {
  return Boolean(
    capabilities?.api.snapshot
      && hasHerdrMethod(capabilities, "pane.get")
      && hasHerdrMethod(capabilities, "pane.scroll")
  )
}

/** The connector scroll command is available to a controller on 0.8.2+. */
export function supportsHerdrTerminalScroll(
  capabilities: HerdrCapabilities | null | undefined
): boolean {
  return Boolean(
    capabilities?.terminal.control
      && capabilities.terminal.resize
      && capabilities.terminal.release
      && capabilities.terminal.scroll
  )
}

export type HerdrScrollStrategy = "pane" | "terminal" | "unavailable"

export function herdrScrollStrategy(
  capabilities: HerdrCapabilities | null | undefined
): HerdrScrollStrategy {
  if (supportsHerdrPaneScroll(capabilities)) return "pane"
  if (supportsHerdrTerminalScroll(capabilities)) return "terminal"
  return "unavailable"
}
