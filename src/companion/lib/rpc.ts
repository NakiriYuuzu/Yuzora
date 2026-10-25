import { fromBase64 } from "./base64url"
import { READ_METHODS, SLOT_METHODS, type Hello } from "./protocol"

/**
 * - `connecting`: opening the socket or waiting for `session.ready`.
 * - `ready`: authenticated, `hello` answered.
 * - `waiting`: closed; a reconnect is scheduled.
 * - `invalid`: the gateway ended the login (4401); only a new login helps.
 */
export type SocketState = "idle" | "connecting" | "ready" | "waiting" | "invalid"

/**
 * A refused or unanswered call. `outcome-unknown` marks a write whose request
 * may have reached the desktop: the socket closed before its answer. Writes
 * are never resent; the user checks the pane and decides.
 */
export class RpcError extends Error {
  constructor(readonly code: string, readonly stream?: number) {
    super(code)
  }
}

type Pending = { method: string; resolve: (value: unknown) => void; reject: (error: RpcError) => void }
type Chunks = { bytes: number; total: number; parts: Uint8Array[] }
type Waiter = () => void

export type SocketOptions = {
  url: string
  binding: () => string | null
  onState: (state: SocketState, closeCode?: number) => void
  onReady: (hello: Hello, epoch: number) => void
  onEvent: (event: string, data: Record<string, unknown>, stream?: number) => void
  onFrame: (stream: number, bytes: Uint8Array) => void
}

const READ_SLOTS = 2
const READ_RETRIES = 3
const AUTH_TIMEOUT = 5000
// The gateway closes a socket that sends 50 messages within a second.
const RATE_LIMIT = 40

export function reconnectDelay(attempt: number, random = Math.random): number {
  const base = Math.min(1000 * 2 ** attempt, 30_000)
  return Math.round(base / 2 + random() * (base / 2))
}

export class CompanionSocket {
  private socket: WebSocket | null = null
  private state: SocketState = "idle"
  private nextId = 1
  private pending = new Map<number, Pending>()
  private chunks = new Map<number, Chunks>()
  private earlyChunks = new Map<number, { index: number; data: string }[]>()
  private sent: number[] = []
  private readsActive = 0
  private readWaiters: Waiter[] = []
  private attempt = 0
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  private authTimer: ReturnType<typeof setTimeout> | null = null
  private epoch = 0
  private stopped = true
  private readonly options: SocketOptions

  constructor(options: SocketOptions) {
    this.options = options
  }

  get current(): SocketState {
    return this.state
  }

  start(): void {
    this.stopped = false
    if (!this.socket && !this.retryTimer) this.open()
  }

  stop(): void {
    this.stopped = true
    this.clearRetry()
    this.socket?.close(1000)
    this.teardown()
    this.setState("idle")
  }

  /** Back in the foreground: skip the remaining backoff. */
  retryNow(): void {
    if (this.stopped || this.state !== "waiting") return
    this.clearRetry()
    this.open()
  }

  /**
   * Reads and `push.subscribe` share two slots (the gateway's per-peer limit)
   * and retry `busy`, which means the gateway did not run the call.
   * Everything else is sent once, immediately, and never retried.
   */
  async call<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (!SLOT_METHODS.has(method)) return this.send<T>(method, params)
    await this.acquireRead()
    try {
      for (let attempt = 0; ; attempt += 1) {
        try {
          return await this.send<T>(method, params)
        } catch (error) {
          if (!(error instanceof RpcError) || error.code !== "busy" || attempt >= READ_RETRIES) throw error
          await new Promise((resolve) => setTimeout(resolve, 300 * 2 ** attempt))
        }
      }
    } finally {
      this.releaseRead()
    }
  }

  private send<T>(method: string, params: Record<string, unknown>): Promise<T> {
    const socket = this.socket
    if (!socket || this.state !== "ready") return Promise.reject(new RpcError("not-connected"))
    const now = Date.now()
    this.sent = this.sent.filter((at) => now - at < 1000)
    if (this.sent.length >= RATE_LIMIT) return Promise.reject(new RpcError("rate-limited"))
    this.sent.push(now)
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { method, resolve: resolve as (value: unknown) => void, reject })
      socket.send(JSON.stringify({ id, method, params }))
    })
  }

  private acquireRead(): Promise<void> {
    if (this.readsActive < READ_SLOTS) {
      this.readsActive += 1
      return Promise.resolve()
    }
    return new Promise((resolve) => this.readWaiters.push(() => {
      this.readsActive += 1
      resolve()
    }))
  }

  private releaseRead(): void {
    this.readsActive -= 1
    this.readWaiters.shift()?.()
  }

  private open(): void {
    const binding = this.options.binding()
    if (!binding) {
      this.setState("invalid")
      return
    }
    this.setState("connecting")
    const socket = new WebSocket(this.options.url)
    socket.binaryType = "arraybuffer"
    this.socket = socket
    socket.onopen = () => socket.send(JSON.stringify({ type: "auth", binding }))
    socket.onmessage = (message) => this.receive(message.data)
    socket.onclose = (event) => {
      if (this.socket === socket) this.closed(event.code)
    }
    this.authTimer = setTimeout(() => {
      if (this.socket === socket && this.state === "connecting") socket.close(4000)
    }, AUTH_TIMEOUT)
  }

  private receive(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      if (data.byteLength < 4) return
      this.options.onFrame(new DataView(data).getUint32(0), new Uint8Array(data, 4))
      return
    }
    let message: { id?: number | null; result?: unknown; error?: { code: string }; event?: string; data?: Record<string, unknown>; stream?: number }
    try {
      message = JSON.parse(String(data))
    } catch {
      return
    }
    if (message.event === "session.ready") {
      this.ready()
      return
    }
    if (message.event === "snapshot.chunk") {
      const chunk = message.data as { id: number; index: number; data: string }
      this.chunk(chunk.id, chunk.index, chunk.data)
      return
    }
    if (message.event) {
      this.options.onEvent(message.event, message.data ?? {}, message.stream)
      return
    }
    if (typeof message.id !== "number") {
      // Binary input results carry id null; nothing here sends binary input.
      return
    }
    const pending = this.pending.get(message.id)
    if (!pending) return
    if (message.error) {
      this.pending.delete(message.id)
      pending.reject(new RpcError(message.error.code, message.stream))
      return
    }
    const result = message.result as { chunked?: boolean; encoding?: string; bytes?: number; chunks?: number }
    if (result?.chunked) {
      if (result.encoding !== "base64-json-v1" || !result.bytes || !result.chunks) {
        this.pending.delete(message.id)
        pending.reject(new RpcError("invalid-response"))
        return
      }
      this.chunks.set(message.id, { bytes: result.bytes, total: result.chunks, parts: [] })
      for (const early of this.earlyChunks.get(message.id) ?? []) this.chunk(message.id, early.index, early.data)
      this.earlyChunks.delete(message.id)
      return
    }
    this.pending.delete(message.id)
    pending.resolve(message.result)
  }

  /** base64-json-v1: decode each chunk to bytes, join, check the length, then decode UTF-8 once. */
  private chunk(id: number, index: number, data: string): void {
    const state = this.chunks.get(id)
    if (!state) {
      const early = this.earlyChunks.get(id) ?? []
      early.push({ index, data })
      this.earlyChunks.set(id, early)
      return
    }
    const pending = this.pending.get(id)
    const fail = () => {
      this.chunks.delete(id)
      this.pending.delete(id)
      pending?.reject(new RpcError("invalid-response"))
    }
    if (index !== state.parts.length) return fail()
    try {
      state.parts.push(fromBase64(data))
    } catch {
      return fail()
    }
    if (state.parts.length < state.total) return
    const joined = new Uint8Array(state.parts.reduce((size, part) => size + part.length, 0))
    let offset = 0
    for (const part of state.parts) {
      joined.set(part, offset)
      offset += part.length
    }
    if (joined.length !== state.bytes) return fail()
    try {
      const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(joined))
      this.chunks.delete(id)
      this.pending.delete(id)
      pending?.resolve(value)
    } catch {
      fail()
    }
  }

  private async ready(): Promise<void> {
    if (this.authTimer) clearTimeout(this.authTimer)
    this.authTimer = null
    const socket = this.socket
    this.setState("ready")
    try {
      const hello = await this.send<Hello>("hello", {})
      if (this.socket !== socket) return
      this.attempt = 0
      this.epoch += 1
      this.options.onReady(hello, this.epoch)
    } catch {
      // The close handler reports the failure.
    }
  }

  private closed(code: number): void {
    this.teardown()
    if (code === 4401) {
      this.setState("invalid", code)
      return
    }
    if (this.stopped) return
    this.setState("waiting", code)
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null
      this.open()
    }, reconnectDelay(this.attempt))
    this.attempt += 1
  }

  private teardown(): void {
    if (this.authTimer) clearTimeout(this.authTimer)
    this.authTimer = null
    const socket = this.socket
    this.socket = null
    if (socket) socket.onclose = socket.onmessage = socket.onopen = null
    for (const pending of this.pending.values()) {
      // A read is safe to ask again; a write may already have happened.
      pending.reject(new RpcError(READ_METHODS.has(pending.method) || pending.method === "hello" ? "disconnected" : "outcome-unknown"))
    }
    this.pending.clear()
    this.chunks.clear()
    this.earlyChunks.clear()
  }

  private clearRetry(): void {
    if (this.retryTimer) clearTimeout(this.retryTimer)
    this.retryTimer = null
  }

  private setState(state: SocketState, closeCode?: number): void {
    this.state = state
    this.options.onState(state, closeCode)
  }
}
