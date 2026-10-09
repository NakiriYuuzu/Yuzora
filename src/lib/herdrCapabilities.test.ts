import { describe, expect, it } from "vitest"
import {
  hasHerdrMethod,
  herdrScrollStrategy,
  herdrScrollStrategyForRuntime,
  supportsHerdrApplicationWheel,
  supportsHerdrPaneScroll,
  supportsHerdrPaneScrollCandidate,
  supportsHerdrTerminalMouse,
  supportsHerdrTerminalScroll
} from "./herdrCapabilities"
import type { HerdrCapabilities } from "./herdrTypes"

function capabilities(methods: string[], overrides: Partial<HerdrCapabilities["terminal"]> = {}): HerdrCapabilities {
  return {
    binaryPath: "/herdr",
    binaryVersion: "0.9.0",
    binaryProtocol: 22,
    channel: null,
    binarySource: {
      configured: "global",
      available: true,
      path: "/herdr",
      restartRequired: false
    },
    server: { running: true, compatible: true },
    api: {
      snapshot: true,
      ping: true,
      tabCreate: true,
      workspaceFocus: true,
      workspaceCreate: true,
      workspaceRename: true,
      workspaceClose: true,
      tabRename: true,
      tabClose: true,
      tabFocus: true,
      paneFocus: true,
      paneRename: true,
      paneSplit: true,
      paneZoom: true,
      paneSwap: true,
      paneClose: true,
      layoutExport: true,
      layoutSetSplitRatio: true,
      eventsSubscribe: true,
      worktreeList: true,
      methods
    },
    terminal: {
      observe: true,
      control: true,
      takeover: true,
      input: true,
      resize: true,
      scroll: true,
      release: true,
      create: true,
      ...overrides
    },
    events: { status: "available" }
  }
}

describe("HERDR capability adapter", () => {
  it("keeps protocol 20 / HERDR 0.8.2 on terminal.scroll when pane.scroll is absent", () => {
    const caps = capabilities(["session.snapshot", "pane.get"])
    caps.binaryVersion = "0.8.2"
    caps.binaryProtocol = 20

    expect(hasHerdrMethod(caps, "pane.get")).toBe(true)
    expect(hasHerdrMethod(caps, "pane.scroll")).toBe(false)
    expect(supportsHerdrPaneScroll(caps)).toBe(false)
    expect(supportsHerdrTerminalScroll(caps)).toBe(true)
    expect(herdrScrollStrategy(caps)).toBe("terminal")
  })

  it("uses pane scroll only when both official pane methods are advertised", () => {
    const caps = capabilities(["session.snapshot", "pane.get", "pane.scroll"])

    expect(supportsHerdrPaneScroll(caps)).toBe(true)
    expect(herdrScrollStrategy(caps)).toBe("pane")
  })

  it("does not infer support from a running server when the capability is unknown", () => {
    const caps = capabilities([], { control: false, resize: false, release: false, scroll: false })

    expect(herdrScrollStrategy(caps)).toBe("unavailable")
  })

  it("blocks the protocol-20 WSL terminal scroll command", () => {
    const caps = capabilities(["session.snapshot", "pane.get"])
    caps.binaryVersion = "0.8.2"
    caps.binaryProtocol = 20
    caps.api.schemaProtocol = 20

    expect(herdrScrollStrategyForRuntime(caps, "wsl:Debian")).toBe("unavailable")
    expect(herdrScrollStrategyForRuntime(caps, "local")).toBe("terminal")
  })

  it("keeps WSL unavailable until its protocol is known", () => {
    const caps = capabilities(["session.snapshot"], { scroll: true })
    caps.binaryProtocol = null
    caps.api.schemaProtocol = null
    caps.server.protocol = null
    expect(herdrScrollStrategyForRuntime(caps, "wsl:Debian")).toBe("unavailable")
  })

  it("uses configured WSL kind and legacy WSL IDs for scroll safety", () => {
    const caps = capabilities([])
    caps.api.schemaProtocol = 20
    caps.binaryProtocol = 20
    expect(herdrScrollStrategyForRuntime(caps, "wsl-ubuntu")).toBe("unavailable")
    expect(herdrScrollStrategyForRuntime(caps, "custom-host", "wsl")).toBe("unavailable")
  })

  it("does not probe from a binary protocol the host pane gate cannot verify", () => {
    const caps = capabilities([])
    caps.api.schemaProtocol = null
    caps.binaryProtocol = 22
    expect(supportsHerdrPaneScrollCandidate(caps)).toBe(false)
    expect(herdrScrollStrategyForRuntime(caps, "wsl:Ubuntu")).toBe("unavailable")
  })

  it("allows a protocol-22 pane probe while the method list is unknown", () => {
    const caps = capabilities([])
    caps.binaryProtocol = 22
    caps.api.schemaProtocol = 22
    expect(supportsHerdrPaneScrollCandidate(caps)).toBe(true)
    expect(herdrScrollStrategyForRuntime(caps, "wsl:Debian")).toBe("pane")
  })

  it("scrolls native protocol-22 runtimes through the pane API like WSL", () => {
    // macOS A/B (2026-09-24): the pane path felt smoother than the paced
    // connector command; its frames were ~4x smaller while scrolling.
    const caps = capabilities(["session.snapshot", "pane.get", "pane.scroll"])

    expect(herdrScrollStrategyForRuntime(caps, "local")).toBe("pane")
    expect(herdrScrollStrategyForRuntime(caps, "windows-native")).toBe("pane")
  })

  it("still probes protocol-22 when the method list is incomplete", () => {
    const caps = capabilities(["session.snapshot", "pane.get"])
    caps.binaryProtocol = 22
    caps.api.schemaProtocol = 22
    expect(supportsHerdrPaneScrollCandidate(caps)).toBe(true)
  })

  it("routes an alternate-screen wheel through the connector on native and protocol-22 WSL", () => {
    const caps = capabilities(["session.snapshot", "pane.get", "pane.scroll"])
    caps.api.schemaProtocol = 22

    expect(herdrScrollStrategyForRuntime(caps, "local")).toBe("pane")
    expect(supportsHerdrApplicationWheel(caps, "local")).toBe(true)
    expect(herdrScrollStrategyForRuntime(caps, "wsl:Debian")).toBe("pane")
    expect(supportsHerdrApplicationWheel(caps, "wsl:Debian")).toBe(true)
    expect(supportsHerdrApplicationWheel(caps, "custom-host", "wsl")).toBe(true)
    expect(supportsHerdrApplicationWheel(capabilities(["pane.get", "pane.scroll"], { scroll: false }), "local")).toBe(false)
  })

  it("keeps the connector wheel off for legacy or unknown-protocol WSL", () => {
    const legacy = capabilities(["session.snapshot", "pane.get"])
    legacy.binaryProtocol = 20
    legacy.api.schemaProtocol = 20
    expect(supportsHerdrApplicationWheel(legacy, "wsl:Debian")).toBe(false)
    expect(supportsHerdrApplicationWheel(legacy, "wsl-ubuntu")).toBe(false)
    expect(supportsHerdrApplicationWheel(legacy, "local")).toBe(true)

    const unknown = capabilities(["session.snapshot"])
    unknown.binaryProtocol = null
    unknown.api.schemaProtocol = null
    unknown.server.protocol = null
    expect(supportsHerdrApplicationWheel(unknown, "wsl:Debian")).toBe(false)
  })

  it("sends terminal.mouse only to 0.9.2+ connectors that can take control", () => {
    const at = (binaryVersion: string | null, overrides: Partial<HerdrCapabilities["terminal"]> = {}) =>
      supportsHerdrTerminalMouse({ ...capabilities([], overrides), binaryVersion })
    expect(at("0.9.1")).toBe(false)
    expect(at("0.9.2")).toBe(true)
    expect(at("v0.9.3")).toBe(true)
    expect(at("0.10.0")).toBe(true)
    expect(at("1.0.0")).toBe(true)
    expect(at("0.8.9")).toBe(false)
    expect(at(null)).toBe(false)
    expect(at("unknown")).toBe(false)
    expect(at("0.9.3", { control: false })).toBe(false)
    expect(supportsHerdrTerminalMouse(null)).toBe(false)
  })
})
