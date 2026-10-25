import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Snapshot } from "./lib/protocol"

// A socket that answers the three reads a refresh makes from `snapshots`.
type SocketOptions = {
  onReady: (hello: unknown, epoch: number) => void
  onEvent: (event: string, data: Record<string, unknown>) => void
  onFrame: (stream: number, bytes: Uint8Array) => void
}
const fake = vi.hoisted(() => ({
  snapshots: {} as Record<string, unknown>,
  sockets: [] as { options: SocketOptions }[],
  calls: [] as string[],
  // Overrides the answer to a call; `undefined` falls through to `snapshots`.
  answer: undefined as ((method: string, params: { session?: string; stream?: number }) => unknown) | undefined,
}))
vi.mock("./lib/rpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./lib/rpc")>()),
  CompanionSocket: class {
    constructor(readonly options: SocketOptions) {
      fake.sockets.push(this)
    }
    start() {}
    stop() {}
    retryNow() {}
    async call(method: string, params: { session?: string; stream?: number }) {
      fake.calls.push(method)
      const answered = fake.answer?.(method, params)
      if (answered !== undefined) return answered
      if (method === "sessions.list") return { sessions: Object.keys(fake.snapshots).map((name) => ({ name, default: false })) }
      if (method === "events.subscribe") return { stream: 1 }
      if (method === "snapshot.get") return fake.snapshots[params.session!]
      if (method === "stream.close") return {}
      throw new Error(method)
    }
  },
}))
import { connect, listen, showPane } from "./client"
import { RpcError } from "./lib/rpc"
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
  fake.calls = []
  fake.answer = undefined
  navigate({ name: "home" })
  useCompanion.setState({ socket: "idle", snapshots: {}, sessionErrors: {}, git: {}, pendingPane: null })
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

describe("a pane whose snapshot failed to load", () => {
  it("stays pending until a refresh proves it gone or shows it", async () => {
    useCompanion.setState({ socket: "ready", pendingPane: null })
    connect()
    fake.snapshots = { work: snapshot("w1:p") }
    let failing = true
    fake.answer = (method) => {
      if (method === "snapshot.get" && failing) throw new RpcError("herdr-unavailable")
    }
    showPane(target)
    await vi.waitFor(() => expect(useCompanion.getState().sessionErrors.work).toBe("herdr-unavailable"))
    expect(useCompanion.getState().pendingPane).toEqual(target)
    expect(screen()).toEqual({ name: "home" })
    failing = false
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 1)
    await vi.waitFor(() => expect(screen()).toEqual(pane))
    expect(useCompanion.getState().pendingPane).toBeNull()
  })
})

describe("events that race the snapshot read", () => {
  const status = (value: string) => ({ session: "work", pane: "w1:p", status: value })
  const withAgent = (value: string): Snapshot => ({
    ...snapshot("w1:p"),
    agents: [{ pane: "w1:p", name: "claude", status: value }] as unknown as Snapshot["agents"],
    panes: [{ id: "w1:p", tabId: null, workspaceId: null, status: value, label: null, layout: null }] as unknown as Snapshot["panes"],
  })

  it("reads the snapshot again when an event landed during the read", async () => {
    useCompanion.setState({ socket: "ready" })
    connect()
    const reads = [withAgent("idle"), withAgent("blocked")]
    let read = 0
    fake.answer = (method) => {
      if (method !== "snapshot.get") return undefined
      const result = reads[Math.min(read++, reads.length - 1)]
      // The event arrives while the first read is on the wire.
      if (read === 1) fake.sockets.at(-1)!.options.onEvent("agent.status", status("blocked"))
      return result
    }
    fake.snapshots = { work: withAgent("idle") }
    // The scheduled reload (500 ms) must not be able to stand in for the
    // immediate re-read: time stays still, only microtasks run.
    vi.useFakeTimers()
    try {
      fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 1)
      await vi.advanceTimersByTimeAsync(0)
      expect(useCompanion.getState().snapshots.work?.panes[0]?.status).toBe("blocked")
      expect(fake.calls.filter((method) => method === "snapshot.get")).toHaveLength(2)
    } finally {
      vi.clearAllTimers()
      vi.useRealTimers()
    }
  })
})

describe("a stream that closes before anyone listens", () => {
  it("tells the listener who registers afterwards", () => {
    connect()
    const { onEvent, onFrame } = fake.sockets.at(-1)!.options
    onFrame(5, new Uint8Array([1]))
    onEvent("stream.closed", { stream: 5, reason: "owned-elsewhere" })
    const data = vi.fn()
    const closed = vi.fn()
    listen(5, { data, closed })
    expect(data).toHaveBeenCalledWith(new Uint8Array([1]))
    expect(closed).toHaveBeenCalledWith("owned-elsewhere")
  })

  it("forgets the early frames of a closure it evicts", () => {
    connect()
    const { onEvent, onFrame } = fake.sockets.at(-1)!.options
    onFrame(100, new Uint8Array([1]))
    onEvent("stream.closed", { stream: 100, reason: "stream-closed" })
    // Sixteen more abandoned streams evict the oldest closure.
    for (let stream = 101; stream <= 116; stream++) onEvent("stream.closed", { stream, reason: "stream-closed" })
    const data = vi.fn()
    const closed = vi.fn()
    listen(100, { data, closed })
    expect(data).not.toHaveBeenCalled()
    expect(closed).not.toHaveBeenCalled()
  })

  it("ignores the closure of a stream it already stopped listening to", () => {
    connect()
    const { onEvent } = fake.sockets.at(-1)!.options
    const release = listen(6, { data() {}, closed() {} })
    release()
    onEvent("stream.closed", { stream: 6, reason: "stream-closed" })
    const closed = vi.fn()
    listen(6, { data() {}, closed })
    expect(closed).not.toHaveBeenCalled()
  })
})

describe("refresh", () => {
  it("closes the subscription of a session that is gone and forgets its error", async () => {
    useCompanion.setState({ socket: "ready", sessionErrors: { old: "busy" } })
    connect()
    fake.snapshots = { old: snapshot() }
    fake.answer = (method) => (method === "events.subscribe" ? { stream: 11 } : undefined)
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 1)
    await vi.waitFor(() => expect(useCompanion.getState().snapshots.old).toBeDefined())
    fake.snapshots = { other: snapshot() }
    fake.calls = []
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 2)
    await vi.waitFor(() => expect(useCompanion.getState().snapshots.other).toBeDefined())
    expect(fake.calls).toContain("stream.close")
    expect(useCompanion.getState().sessionErrors.old).toBeUndefined()
  })

  it("leaves the gateway's stream room for a pane's terminal streams and still reads every session", async () => {
    useCompanion.setState({ socket: "ready" })
    connect()
    const names = Array.from({ length: 9 }, (_, index) => `s${index}`)
    fake.snapshots = Object.fromEntries(names.map((name) => [name, { ...snapshot(), session: name }]))
    const subscribed: string[] = []
    fake.answer = (method, params) => {
      if (method !== "events.subscribe") return undefined
      subscribed.push(params.session!)
      return { stream: 20 + subscribed.length }
    }
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z", capabilities: { limits: { streams: 8 } } }, 1)
    await vi.waitFor(() => expect(Object.keys(useCompanion.getState().snapshots).sort()).toEqual(names))
    expect(subscribed).toEqual(names.slice(0, 6))
    expect(useCompanion.getState().sessionErrors).toEqual({})
  })

  it("reads a session whose subscription the gateway refused for the stream limit", async () => {
    useCompanion.setState({ socket: "ready" })
    connect()
    fake.snapshots = { work: snapshot("w1:p") }
    fake.answer = (method) => {
      if (method === "events.subscribe") throw new RpcError("stream-limit")
    }
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z", capabilities: { limits: { streams: 8 } } }, 1)
    await vi.waitFor(() => expect(useCompanion.getState().snapshots.work).toBeDefined())
    expect(useCompanion.getState().sessionErrors.work).toBeUndefined()
  })
})

describe("the Git caches follow the snapshots", () => {
  const spaces = (...workspaces: { id: string; git: boolean; gitGeneration?: string }[]): Snapshot => ({
    ...snapshot(),
    workspaces: workspaces.map((space) => ({ label: null, status: "idle" as const, ...space })),
  })
  const cached = (generation: string | undefined) => ({ generation, commits: {} })
  const key = (session: string, workspace: string) => `${session}\0${workspace}`
  const reload = async (session: string, snapshots: Record<string, Snapshot>) => {
    fake.snapshots = snapshots
    fake.sockets.at(-1)!.options.onReady({ serverTime: "2026-10-20T00:00:00Z" }, 1)
    await vi.waitFor(() => expect(useCompanion.getState().snapshots[session]).toBe(snapshots[session]))
  }
  const seeded = () => {
    useCompanion.setState({
      socket: "ready",
      git: {
        [key("work", "w1")]: cached("g1"),
        [key("work", "w2")]: cached("g1"),
        [key("work", "w3")]: cached("g1"),
        [key("work", "w4")]: cached("g1"),
        [key("other", "w1")]: cached("o1"),
      },
    })
    connect()
  }

  it("drops a Space's reads when the desktop maps it to another repository, and only then", async () => {
    seeded()
    await reload("work", { work: spaces({ id: "w1", git: true, gitGeneration: "g2" }, { id: "w2", git: true, gitGeneration: "g1" }, { id: "w3", git: true, gitGeneration: "g1" }, { id: "w4", git: true, gitGeneration: "g1" }), other: spaces({ id: "w1", git: true, gitGeneration: "o1" }) })
    const keys = Object.keys(useCompanion.getState().git)
    expect(keys).not.toContain(key("work", "w1"))
    expect(keys).toContain(key("work", "w2"))
    // Another session's Spaces are not this snapshot's business.
    expect(keys).toContain(key("other", "w1"))
  })

  it("drops the reads of a Space that disappeared or stopped sharing its repository", async () => {
    seeded()
    await reload("work", { work: spaces({ id: "w1", git: true, gitGeneration: "g1" }, { id: "w2", git: false }, { id: "w3", git: true, gitGeneration: "g1" }), other: spaces({ id: "w1", git: true, gitGeneration: "o1" }) })
    expect(Object.keys(useCompanion.getState().git).sort()).toEqual([key("other", "w1"), key("work", "w1"), key("work", "w3")].sort())
  })

  it("drops the reads of a session that is gone", async () => {
    seeded()
    await reload("other", { other: spaces({ id: "w1", git: true, gitGeneration: "o1" }) })
    expect(Object.keys(useCompanion.getState().git).every((entry) => entry.startsWith("other\0"))).toBe(true)
    expect(Object.keys(useCompanion.getState().git)).toEqual([key("other", "w1")])
  })
})
