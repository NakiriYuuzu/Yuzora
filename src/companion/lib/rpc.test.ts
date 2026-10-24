import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest"
import { CompanionSocket, reconnectDelay, RpcError, type SocketOptions } from "./rpc"

class FakeSocket {
  static all: FakeSocket[] = []
  binaryType = ""
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: ((event: { code: number }) => void) | null = null
  constructor(readonly url: string) {
    FakeSocket.all.push(this)
  }
  send(data: string) {
    this.sent.push(data)
  }
  close(code = 1000) {
    this.onclose?.({ code })
  }
  server(message: unknown) {
    this.onmessage?.({ data: typeof message === "string" || message instanceof ArrayBuffer ? message : JSON.stringify(message) })
  }
  calls(): { id: number; method: string; params: Record<string, unknown> }[] {
    return this.sent.map((text) => JSON.parse(text)).filter((frame) => "method" in frame)
  }
  reply(method: string, result: unknown, nth = 0) {
    const call = this.calls().filter((frame) => frame.method === method)[nth]
    this.server({ id: call.id, result })
  }
}

type Handlers = { [K in "onState" | "onReady" | "onEvent" | "onFrame"]: Mock<SocketOptions[K]> }
let options: SocketOptions & Handlers
let binding: string | null

beforeEach(() => {
  FakeSocket.all = []
  binding = "B"
  vi.stubGlobal("WebSocket", FakeSocket)
  options = {
    url: "wss://desktop/ws",
    binding: () => binding,
    onState: vi.fn<SocketOptions["onState"]>(),
    onReady: vi.fn<SocketOptions["onReady"]>(),
    onEvent: vi.fn<SocketOptions["onEvent"]>(),
    onFrame: vi.fn<SocketOptions["onFrame"]>(),
  }
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

async function connected(): Promise<[CompanionSocket, FakeSocket]> {
  const client = new CompanionSocket(options)
  client.start()
  const socket = FakeSocket.all.at(-1)!
  socket.onopen?.()
  socket.server({ event: "session.ready", data: { protocolVersion: 1 } })
  socket.reply("hello", { protocolVersion: 1, serverTime: "2026-10-18T00:00:00Z", scopes: ["view"] })
  await flush()
  return [client, socket]
}

describe("CompanionSocket", () => {
  it("authenticates with the binding as the first frame, then says hello", async () => {
    const [client, socket] = await connected()
    expect(JSON.parse(socket.sent[0])).toEqual({ type: "auth", binding: "B" })
    expect(socket.calls()[0].method).toBe("hello")
    expect(client.current).toBe("ready")
    expect(options.onReady).toHaveBeenCalledWith(expect.objectContaining({ scopes: ["view"] }), 1)
    expect(socket.binaryType).toBe("arraybuffer")
  })

  it("does not connect without a binding", () => {
    binding = null
    new CompanionSocket(options).start()
    expect(FakeSocket.all).toHaveLength(0)
    expect(options.onState).toHaveBeenLastCalledWith("invalid", undefined)
  })

  it("resolves results and rejects errors by request id", async () => {
    const [client, socket] = await connected()
    const sessions = client.call("sessions.list")
    const prompt = client.call("agent.prompt", { session: "s", target: "p", text: "hi" })
    await flush()
    socket.reply("sessions.list", { sessions: [{ name: "work", default: true }] })
    const call = socket.calls().find((frame) => frame.method === "agent.prompt")!
    socket.server({ id: call.id, error: { code: "elevation-required", message: "x" } })
    await expect(sessions).resolves.toEqual({ sessions: [{ name: "work", default: true }] })
    await expect(prompt).rejects.toMatchObject({ code: "elevation-required" })
  })

  it("reassembles a chunked snapshot from bytes, even when UTF-8 splits across chunks", async () => {
    const [client, socket] = await connected()
    const value = { session: "work", label: "日本語".repeat(10) }
    const bytes = new TextEncoder().encode(JSON.stringify(value))
    const parts = [bytes.slice(0, 20), bytes.slice(20, 41), bytes.slice(41)]
    const result = client.call("snapshot.get", { session: "work" })
    await flush()
    const id = socket.calls().find((frame) => frame.method === "snapshot.get")!.id
    socket.server({ id, result: { chunked: true, encoding: "base64-json-v1", bytes: bytes.length, chunks: 3 } })
    parts.forEach((part, index) => socket.server({ event: "snapshot.chunk", data: { id, index, data: btoa(String.fromCharCode(...part)) } }))
    await expect(result).resolves.toEqual(value)
  })

  it("rejects a chunked snapshot whose length does not match", async () => {
    const [client, socket] = await connected()
    const result = client.call("snapshot.get", { session: "work" })
    await flush()
    const id = socket.calls().find((frame) => frame.method === "snapshot.get")!.id
    socket.server({ id, result: { chunked: true, encoding: "base64-json-v1", bytes: 99, chunks: 1 } })
    socket.server({ event: "snapshot.chunk", data: { id, index: 0, data: btoa("{}") } })
    await expect(result).rejects.toMatchObject({ code: "invalid-response" })
  })

  it("keeps at most two reads in flight and retries only busy reads", async () => {
    vi.useFakeTimers()
    const [client, socket] = await (async () => {
      const result = connected()
      await vi.advanceTimersByTimeAsync(0)
      return result
    })()
    const reads = ["a", "b", "c"].map((session) => client.call("snapshot.get", { session }))
    await vi.advanceTimersByTimeAsync(0)
    expect(socket.calls().filter((frame) => frame.method === "snapshot.get")).toHaveLength(2)
    const first = socket.calls().find((frame) => frame.method === "snapshot.get")!
    socket.server({ id: first.id, error: { code: "busy", message: "busy" } })
    await vi.advanceTimersByTimeAsync(300)
    const retried = socket.calls().filter((frame) => frame.method === "snapshot.get")
    expect(retried).toHaveLength(3)
    expect(retried[2].params).toEqual({ session: "a" })
    for (const frame of retried.slice(1)) socket.server({ id: frame.id, result: { ok: frame.params.session } })
    await vi.advanceTimersByTimeAsync(0)
    const third = socket.calls().filter((frame) => frame.method === "snapshot.get")[3]
    socket.server({ id: third.id, result: { ok: "c" } })
    await expect(Promise.all(reads)).resolves.toEqual([{ ok: "a" }, { ok: "b" }, { ok: "c" }])

    const write = client.call("pane.sendKeys", { session: "s", target: "p", keys: ["Enter"] })
    await vi.advanceTimersByTimeAsync(0)
    const keys = socket.calls().find((frame) => frame.method === "pane.sendKeys")!
    socket.server({ id: keys.id, error: { code: "busy", message: "busy" } })
    await expect(write).rejects.toMatchObject({ code: "busy" })
    await vi.advanceTimersByTimeAsync(5000)
    expect(socket.calls().filter((frame) => frame.method === "pane.sendKeys")).toHaveLength(1)
  })

  it("never resends a write the socket lost and marks its outcome unknown", async () => {
    vi.useFakeTimers()
    const ready = connected()
    await vi.advanceTimersByTimeAsync(0)
    const [client, socket] = await ready
    const input = client.call("terminal.input", { stream: 3, text: "y" })
    const read = client.call("sessions.list")
    await vi.advanceTimersByTimeAsync(0)
    socket.close(1006)
    await expect(input).rejects.toMatchObject({ code: "outcome-unknown" })
    await expect(read).rejects.toMatchObject({ code: "disconnected" })
    expect(options.onState).toHaveBeenLastCalledWith("waiting", 1006)
    await expect(client.call("terminal.input", { stream: 3, text: "y" })).rejects.toMatchObject({ code: "not-connected" })

    await vi.advanceTimersByTimeAsync(1000)
    const next = FakeSocket.all.at(-1)!
    expect(next).not.toBe(socket)
    next.onopen?.()
    next.server({ event: "session.ready", data: { protocolVersion: 1 } })
    next.reply("hello", { protocolVersion: 1, serverTime: "2026-10-18T00:00:00Z", scopes: ["view"] })
    await vi.advanceTimersByTimeAsync(0)
    expect(options.onReady).toHaveBeenLastCalledWith(expect.anything(), 2)
    expect(next.calls().map((frame) => frame.method)).toEqual(["hello"])
  })

  it("drops input typed while not connected: nothing is queued and nothing is replayed on the next socket", async () => {
    vi.useFakeTimers()
    const writes: [string, Record<string, unknown>][] = [
      ["terminal.input", { stream: 3, text: "rm -rf build\r" }],
      ["agent.prompt", { session: "work", target: "w:p", text: "approve" }],
      ["pane.sendKeys", { session: "work", target: "w:p", keys: ["Enter"] }],
      ["full.input", { stream: 4, text: "y" }],
      ["terminal.control", { session: "work", target: "w:p" }],
    ]
    const ready = connected()
    await vi.advanceTimersByTimeAsync(0)
    const [client, socket] = await ready
    socket.close(1006) // the network dropped; a reconnect is scheduled
    expect(options.onState).toHaveBeenLastCalledWith("waiting", 1006)
    for (const [method, params] of writes) {
      await expect(client.call(method, params)).rejects.toMatchObject({ code: "not-connected" })
    }

    await vi.advanceTimersByTimeAsync(1000)
    const next = FakeSocket.all.at(-1)!
    expect(next).not.toBe(socket)
    next.onopen?.() // authenticating: still not ready
    for (const [method, params] of writes) {
      await expect(client.call(method, params)).rejects.toMatchObject({ code: "not-connected" })
    }
    expect(next.sent.map((text) => JSON.parse(text))).toEqual([{ type: "auth", binding: "B" }])
    next.server({ event: "session.ready", data: { protocolVersion: 1 } })
    next.reply("hello", { protocolVersion: 1, serverTime: "2026-10-18T00:00:00Z", scopes: ["view"] })
    await vi.advanceTimersByTimeAsync(5000)
    // Only auth and hello ever reach the new socket: no dropped write was kept.
    expect(next.sent.map((text) => JSON.parse(text).method ?? "auth")).toEqual(["auth", "hello"])
    expect(socket.sent.map((text) => JSON.parse(text).method ?? "auth")).toEqual(["auth", "hello"])
  })

  it("backs off up to 30 s with jitter and retries at once when asked", async () => {
    expect(reconnectDelay(0, () => 0)).toBe(500)
    expect(reconnectDelay(0, () => 1)).toBe(1000)
    expect(reconnectDelay(3, () => 1)).toBe(8000)
    expect(reconnectDelay(10, () => 1)).toBe(30_000)
    vi.useFakeTimers()
    const client = new CompanionSocket(options)
    client.start()
    for (let attempt = 0; attempt < 4; attempt += 1) {
      FakeSocket.all.at(-1)!.close(1006)
      await vi.advanceTimersByTimeAsync(reconnectDelay(attempt, () => 1))
    }
    const count = FakeSocket.all.length
    FakeSocket.all.at(-1)!.close(1006)
    client.retryNow()
    expect(FakeSocket.all.length).toBe(count + 1)
    client.start()
    expect(FakeSocket.all.length).toBe(count + 1)
  })

  it("stops for good when the gateway ends the login", async () => {
    vi.useFakeTimers()
    const ready = connected()
    await vi.advanceTimersByTimeAsync(0)
    const [client, socket] = await ready
    socket.close(4401)
    expect(client.current).toBe("invalid")
    await vi.advanceTimersByTimeAsync(60_000)
    expect(FakeSocket.all).toHaveLength(1)
  })

  it("closes a socket that never confirms the login", async () => {
    vi.useFakeTimers()
    new CompanionSocket(options).start()
    FakeSocket.all[0].onopen?.()
    await vi.advanceTimersByTimeAsync(5000)
    expect(options.onState).toHaveBeenCalledWith("waiting", 4000)
  })

  it("routes stream frames and events", async () => {
    const [, socket] = await connected()
    const frame = new Uint8Array([0, 0, 0, 7, 104, 105]).buffer
    socket.server(frame)
    expect(options.onFrame).toHaveBeenCalledWith(7, new Uint8Array([104, 105]))
    socket.server({ event: "stream.closed", data: { stream: 7, reason: "taken-over" } })
    expect(options.onEvent).toHaveBeenCalledWith("stream.closed", { stream: 7, reason: "taken-over" }, undefined)
    socket.server({ event: "agent.status", data: { pane: "p", status: "blocked" }, stream: 1 })
    expect(options.onEvent).toHaveBeenLastCalledWith("agent.status", { pane: "p", status: "blocked" }, 1)
  })

  it("refuses sends locally before the gateway's message-rate limit", async () => {
    const [client] = await connected()
    const codes: string[] = []
    for (let index = 0; index < 45; index += 1) client.call("session.ping").catch((error: RpcError) => codes.push(error.code))
    await flush()
    // hello took one of the 40 slots.
    expect(codes).toEqual(Array(6).fill("rate-limited"))
  })
})
