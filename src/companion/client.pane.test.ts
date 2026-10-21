import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Snapshot } from "./lib/protocol"

// A socket that answers the three reads a refresh makes from `snapshots`.
const fake = vi.hoisted(() => ({
  snapshots: {} as Record<string, unknown>,
  sockets: [] as { options: { onReady: (hello: unknown, epoch: number) => void } }[],
}))
vi.mock("./lib/rpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./lib/rpc")>()),
  CompanionSocket: class {
    constructor(readonly options: { onReady: (hello: unknown, epoch: number) => void }) {
      fake.sockets.push(this)
    }
    start() {}
    stop() {}
    retryNow() {}
    async call(method: string, params: { session?: string }) {
      if (method === "sessions.list") return { sessions: Object.keys(fake.snapshots).map((name) => ({ name, default: false })) }
      if (method === "events.subscribe") return { stream: 1 }
      if (method === "snapshot.get") return fake.snapshots[params.session!]
      throw new Error(method)
    }
  },
}))
import { connect, showPane } from "./client"
import { navigate, useCompanion } from "./store"

const snapshot = (...panes: string[]): Snapshot => ({
  session: "work",
  workspaces: [],
  tabs: [],
  panes: panes.map((id) => ({ id, tabId: null, workspaceId: null, status: "idle", label: null, layout: null })),
  agents: [],
})
const target = { session: "work", pane: "w1:p" }
const screen = () => useCompanion.getState().screen
const pane = { name: "pane", session: "work", pane: "w1:p" }

beforeEach(() => {
  fake.snapshots = {}
  navigate({ name: "home" })
  useCompanion.setState({ socket: "idle", snapshots: {}, pendingPane: null })
})
afterEach(() => vi.restoreAllMocks())

describe("showPane", () => {
  it("opens a pane the snapshots already show", () => {
    useCompanion.setState({ socket: "ready", snapshots: { work: snapshot("w1:p") } })
    showPane(target)
    expect(screen()).toEqual(pane)
    expect(useCompanion.getState().pendingPane).toBeNull()
  })

  it("does not open the pane it is already showing again", () => {
    useCompanion.setState({ socket: "ready", snapshots: { work: snapshot("w1:p") } })
    showPane(target)
    const history = vi.spyOn(window.history, "pushState")
    showPane(target)
    expect(history).not.toHaveBeenCalled()
  })

  it("waits for a connection that is not up yet", () => {
    showPane(target)
    expect(screen()).toEqual({ name: "home" })
    expect(useCompanion.getState().pendingPane).toEqual(target)
  })

  it("refreshes a connected app whose snapshots predate the pane, then opens it", async () => {
    useCompanion.setState({ socket: "ready", snapshots: { work: snapshot("w0:p") } })
    connect()
    fake.snapshots = { work: snapshot("w0:p", "w1:p") }
    showPane(target)
    await vi.waitFor(() => expect(screen()).toEqual(pane))
    expect(useCompanion.getState().pendingPane).toBeNull()
  })

  it("opens it after the first load of a cold start", async () => {
    useCompanion.setState({ pendingPane: target })
    connect()
    fake.snapshots = { work: snapshot("w1:p") }
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 1)
    await vi.waitFor(() => expect(screen()).toEqual(pane))
  })

  it("drops a pane that is gone once the snapshots are in", async () => {
    useCompanion.setState({ socket: "ready", pendingPane: null })
    connect()
    fake.snapshots = { work: snapshot("w0:p") }
    showPane(target)
    await vi.waitFor(() => expect(useCompanion.getState().snapshots.work).toBeDefined())
    await vi.waitFor(() => expect(useCompanion.getState().pendingPane).toBeNull())
    expect(screen()).toEqual({ name: "home" })
  })

  it("drops a pane of a session that no longer exists", async () => {
    useCompanion.setState({ socket: "ready" })
    connect()
    fake.snapshots = { other: snapshot("w1:p") }
    showPane(target)
    await vi.waitFor(() => expect(useCompanion.getState().pendingPane).toBeNull())
    expect(screen()).toEqual({ name: "home" })
  })
})
