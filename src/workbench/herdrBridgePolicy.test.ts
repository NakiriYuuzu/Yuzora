import { describe, expect, it } from "vitest"

import type { HerdrCapabilities } from "@/lib/herdrTypes"
import {
  shouldPollHerdrSnapshots,
  shouldRefreshWorktreeInventory
} from "@/workbench/herdrBridgePolicy"

const base = {
  binarySource: {
    configured: "global",
    resolved: "global",
    available: true,
    path: "/bin/herdr",
    reason: null,
    restartRequired: false
  },
  server: { running: true },
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
    methods: ["agent.get", "agent.read", "events.subscribe"]
  },
  terminal: {
    observe: true,
    control: true,
    takeover: true,
    input: true,
    resize: true,
    scroll: true,
    release: true,
    create: true
  },
  events: { status: "available" as const }
} satisfies HerdrCapabilities

describe("shouldPollHerdrSnapshots", () => {
  it("polls when events are unavailable or not healthy", () => {
    expect(shouldPollHerdrSnapshots(base, false)).toBe(true)
    expect(
      shouldPollHerdrSnapshots(
        { ...base, events: { status: "unavailable" } },
        true
      )
    ).toBe(true)
  })

  it("suppresses frequent polling when events are available and healthy", () => {
    expect(shouldPollHerdrSnapshots(base, true)).toBe(false)
    expect(shouldPollHerdrSnapshots(base, true, 11_999, 12_000)).toBe(false)
  })

  it("keeps a low-frequency fallback while events are healthy", () => {
    expect(shouldPollHerdrSnapshots(base, true, 12_000, 12_000)).toBe(true)
  })
})

describe("shouldRefreshWorktreeInventory", () => {
  it("keeps a low-frequency authoritative fallback when worktree.list is available", () => {
    expect(shouldRefreshWorktreeInventory(base, 29_999, 30_000)).toBe(false)
    expect(shouldRefreshWorktreeInventory(base, 30_000, 30_000)).toBe(true)
    expect(
      shouldRefreshWorktreeInventory(
        { ...base, api: { ...base.api, worktreeList: false } },
        30_000,
        30_000
      )
    ).toBe(false)
  })
})
