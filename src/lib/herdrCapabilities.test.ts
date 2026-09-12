import { describe, expect, it } from "vitest"
import {
  hasHerdrMethod,
  herdrScrollStrategy,
  supportsHerdrPaneScroll,
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
      agentGet: true,
      agentRead: true,
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
})
