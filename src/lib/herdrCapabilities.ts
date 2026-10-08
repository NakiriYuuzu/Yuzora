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

/**
 * Protocol 22 is the pane-scroll schema boundary. During remote host
 * reconnects an otherwise valid schema can arrive without its method list;
 * treat that state as a probe candidate so the official pane endpoint can
 * establish the real range. Match the host's advertised-method/schema gate;
 * a binary/server version alone cannot authorize the probe. An unsupported
 * endpoint simply leaves the rendered proxy disabled.
 */
export function supportsHerdrPaneScrollCandidate(
  capabilities: HerdrCapabilities | null | undefined
): boolean {
  if (!capabilities?.api.snapshot) return false
  return supportsHerdrPaneScroll(capabilities)
    || (capabilities.api.schemaProtocol ?? 0) >= 22
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

function isWslRuntime(hostId?: string | null, hostKind?: "ssh" | "wsl"): boolean {
  return hostKind === "wsl" || (!hostKind && /^wsl[:-]/i.test(hostId ?? ""))
}

function runtimeProtocol(capabilities: HerdrCapabilities | null | undefined): number | null | undefined {
  return capabilities?.api.schemaProtocol
    ?? capabilities?.binaryProtocol
    ?? capabilities?.server.protocol
}

/**
 * The WSL bridge shipped with protocol 20 advertises terminal control but
 * can terminate the connector when it receives `terminal.scroll`. Keep that
 * legacy boundary unavailable until the runtime exposes the pane-owned scroll
 * API (protocol 22). A missing protocol is also treated as unsafe for WSL so
 * an incomplete capability response cannot trigger the destructive command.
 */
export function herdrScrollStrategyForRuntime(
  capabilities: HerdrCapabilities | null | undefined,
  hostId?: string | null,
  hostKind?: "ssh" | "wsl"
): HerdrScrollStrategy {
  if (isWslRuntime(hostId, hostKind)) {
    const protocol = runtimeProtocol(capabilities)
    if (protocol == null || protocol < 22) return "unavailable"
    // The WSL bridge historically tears down its connector when receiving
    // terminal.scroll. Protocol 22 is the boundary where the pane-owned API
    // is safe to use for that bridge.
    return supportsHerdrPaneScrollCandidate(capabilities) ? "pane" : "unavailable"
  }
  // Native desktop HERDR uses the same pane API as WSL when it is available:
  // absolute "latest offset wins" writes drive the scrollbar. Physical wheels
  // still go through the connector where supportsHerdrApplicationWheel allows
  // it, because only HERDR knows whether the child owns the wheel.
  if (supportsHerdrPaneScrollCandidate(capabilities)) return "pane"
  if (supportsHerdrTerminalScroll(capabilities)) return "terminal"
  return herdrScrollStrategy(capabilities)
}

/**
 * `pane.scroll` only moves host scrollback; it never reaches a TUI that owns
 * the wheel (Claude Code fullscreen, vim, less, or a normal-buffer app with
 * mouse reporting). The connector command lets HERDR route every physical
 * wheel like its official client: mouse report, alternate-scroll keys or
 * host scrollback. WSL keeps the protocol 22 boundary above: the
 * connector-closing `terminal.scroll` report predates that runtime.
 */
export function supportsHerdrApplicationWheel(
  capabilities: HerdrCapabilities | null | undefined,
  hostId?: string | null,
  hostKind?: "ssh" | "wsl"
): boolean {
  if (!supportsHerdrTerminalScroll(capabilities)) return false
  if (!isWslRuntime(hostId, hostKind)) return true
  const protocol = runtimeProtocol(capabilities)
  return protocol != null && protocol >= 22
}

/**
 * `terminal.mouse` is a connector command parsed by the selected HERDR
 * binary, so its version is the only signal: connectors before 0.9.2 print
 * an ignored-command error for every event, which surfaces as a terminal error.
 */
export function supportsHerdrTerminalMouse(
  capabilities: HerdrCapabilities | null | undefined
): boolean {
  if (!capabilities?.terminal.control || !capabilities.terminal.input) return false
  const match = /^v?(\d+)\.(\d+)\.(\d+)/.exec(capabilities.binaryVersion?.trim() ?? "")
  if (!match) return false
  const [major, minor, patch] = match.slice(1).map(Number)
  if (major !== 0) return major > 0
  return minor > 9 || (minor === 9 && patch >= 2)
}

export function herdrScrollStrategy(
  capabilities: HerdrCapabilities | null | undefined
): HerdrScrollStrategy {
  if (supportsHerdrPaneScroll(capabilities)) return "pane"
  if (supportsHerdrTerminalScroll(capabilities)) return "terminal"
  return "unavailable"
}
