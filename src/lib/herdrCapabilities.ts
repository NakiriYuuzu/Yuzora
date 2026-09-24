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
  if (hostKind === "wsl" || (!hostKind && /^wsl[:-]/i.test(hostId ?? ""))) {
    const protocol = capabilities?.api.schemaProtocol
      ?? capabilities?.binaryProtocol
      ?? capabilities?.server.protocol
    if (protocol == null || protocol < 22) return "unavailable"
    // The WSL bridge historically tears down its connector when receiving
    // terminal.scroll. Protocol 22 is the boundary where the pane-owned API
    // is safe to use for that bridge.
    return supportsHerdrPaneScrollCandidate(capabilities) ? "pane" : "unavailable"
  }
  // Native desktop HERDR uses the same pane API as WSL when it is available:
  // absolute "latest offset wins" writes at a steady cadence felt smoother on
  // macOS than the relative connector command, which remains the fallback for
  // runtimes without the pane API (frame-paced in the transport).
  if (supportsHerdrPaneScrollCandidate(capabilities)) return "pane"
  if (supportsHerdrTerminalScroll(capabilities)) return "terminal"
  return herdrScrollStrategy(capabilities)
}

export function herdrScrollStrategy(
  capabilities: HerdrCapabilities | null | undefined
): HerdrScrollStrategy {
  if (supportsHerdrPaneScroll(capabilities)) return "pane"
  if (supportsHerdrTerminalScroll(capabilities)) return "terminal"
  return "unavailable"
}
