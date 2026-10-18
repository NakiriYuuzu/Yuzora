import { withAgentStatus, withoutPane } from "./lib/agents"
import { api } from "./lib/api"
import type { AgentStatus, Hello, SessionInfo, Snapshot, StreamOpen } from "./lib/protocol"
import { CompanionSocket, RpcError } from "./lib/rpc"
import { clockOffset } from "./lib/time"
import { navigate, storage, useCompanion } from "./store"

type StreamHandlers = { data: (bytes: Uint8Array) => void; closed: (reason: string) => void }

const streams = new Map<number, StreamHandlers>()
// Frames that arrive before their stream's listener is registered.
const early = new Map<number, Uint8Array[]>()
const EARLY_FRAMES = 64
// Streams this page stopped listening to; their late frames are dropped.
const ended = new Set<number>()
// Each session's event subscription, and its pending re-subscription.
const eventStreams = new Map<string, number>()
const reloads = new Map<string, ReturnType<typeof setTimeout>>()
const lastReload = new Map<string, number>()
let socket: CompanionSocket | null = null
let lastPing = 0

function socketUrl(): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`
}

/** Connects with the stored binding; a 4401 sends the user back to login. */
export function connect(): void {
  socket?.stop()
  streams.clear()
  socket = new CompanionSocket({
    url: socketUrl(),
    binding: storage.binding,
    onState(state) {
      useCompanion.setState({ socket: state })
      if (state === "invalid") {
        socket?.stop()
        socket = null
        endSession("expired")
      }
      if (state === "waiting") {
        for (const [, handlers] of streams) handlers.closed("disconnected")
        streams.clear()
        early.clear()
        ended.clear()
        eventStreams.clear()
      }
    },
    onReady(hello: Hello, epoch: number) {
      useCompanion.setState({ hello, epoch, offset: clockOffset(hello.serverTime) })
      void refresh()
    },
    onEvent(event, data) {
      if (event === "agent.status") applyStatus(String(data.session), String(data.pane), data.status as AgentStatus)
      else if (event === "pane.exited") applyExit(String(data.session), String(data.pane))
      else if (event === "snapshot.stale") scheduleReload(String(data.session))
      else if (event === "stream.closed") {
        const stream = Number(data.stream)
        const handlers = streams.get(stream)
        streams.delete(stream)
        early.delete(stream)
        handlers?.closed(String(data.reason))
        for (const [session, id] of eventStreams) {
          if (id !== stream) continue
          eventStreams.delete(session)
          scheduleReload(session)
        }
      }
    },
    onFrame(stream, bytes) {
      const handlers = streams.get(stream)
      if (handlers) return handlers.data(bytes)
      if (ended.has(stream)) return
      const queue = early.get(stream) ?? []
      if (queue.length < EARLY_FRAMES) early.set(stream, [...queue, bytes.slice()])
    },
  })
  socket.start()
}

export function disconnect(): void {
  socket?.stop()
  socket = null
  streams.clear()
  early.clear()
  ended.clear()
  eventStreams.clear()
  for (const timer of reloads.values()) clearTimeout(timer)
  reloads.clear()
}

export function call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  if (!socket) return Promise.reject(new RpcError("not-connected"))
  return socket.call<T>(method, params)
}

/** Registers a stream's frames and closure; `release` stops listening. */
export function listen(stream: number, handlers: StreamHandlers): () => void {
  streams.set(stream, handlers)
  for (const bytes of early.get(stream) ?? []) handlers.data(bytes)
  early.delete(stream)
  return () => {
    if (streams.get(stream) !== handlers) return
    streams.delete(stream)
    ended.add(stream)
  }
}

export async function observe(session: string, pane: string): Promise<StreamOpen> {
  return call<StreamOpen>("terminal.observe", { session, target: pane, cols: 1, rows: 1 })
}

export function retryNow(): void {
  socket?.retryNow()
}

/**
 * Sessions, then per session an event subscription and a snapshot. Events
 * only report status changes and exits, so this also runs on reconnect and on
 * return to the foreground; `snapshot.stale` re-subscribes a single session.
 */
export async function refresh(): Promise<void> {
  let sessions: SessionInfo[]
  try {
    sessions = (await call<{ sessions: SessionInfo[] }>("sessions.list")).sessions
  } catch {
    return
  }
  sessions.sort((left, right) => Number(right.default) - Number(left.default))
  useCompanion.setState((state) => ({
    sessions,
    snapshots: Object.fromEntries(Object.entries(state.snapshots).filter(([name]) => sessions.some((item) => item.name === name))),
  }))
  await Promise.all(sessions.map((session) => loadSession(session.name, false)))
}

// The gateway scopes agent-status events to the panes that existed when the
// subscription opened, so a topology change needs a fresh subscription.
function scheduleReload(session: string): void {
  if (reloads.has(session)) return
  // A subscription that keeps closing must not turn into a request loop.
  const delay = Date.now() - (lastReload.get(session) ?? 0) < 5000 ? 5000 : 500
  reloads.set(session, setTimeout(() => {
    reloads.delete(session)
    lastReload.set(session, Date.now())
    void loadSession(session, true)
  }, delay))
}

async function loadSession(name: string, resubscribe: boolean): Promise<void> {
  try {
    const previous = eventStreams.get(name)
    if (resubscribe && previous !== undefined) {
      eventStreams.delete(name)
      await call("stream.close", { stream: previous }).catch(() => undefined)
    }
    // Subscribe first: the snapshot that follows already includes anything the
    // subscription missed.
    const subscribed = await call<{ stream: number }>("events.subscribe", { session: name })
    eventStreams.set(name, subscribed.stream)
    const snapshot = await call<Snapshot>("snapshot.get", { session: name })
    useCompanion.setState((state) => ({ snapshots: { ...state.snapshots, [name]: snapshot }, sessionErrors: withoutKey(state.sessionErrors, name) }))
  } catch (error) {
    if (error instanceof RpcError && error.code !== "disconnected") {
      useCompanion.setState((state) => ({ sessionErrors: { ...state.sessionErrors, [name]: error.code } }))
    }
  }
}

function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record }
  delete next[key]
  return next
}

function applyStatus(session: string, pane: string, status: AgentStatus): void {
  const snapshot = useCompanion.getState().snapshots[session]
  if (!snapshot) return
  const next = withAgentStatus(snapshot, pane, status)
  if (next) useCompanion.setState((state) => ({ snapshots: { ...state.snapshots, [session]: next } }))
  // A pane that became an agent after the snapshot.
  else scheduleReload(session)
}

function applyExit(session: string, pane: string): void {
  const snapshot = useCompanion.getState().snapshots[session]
  if (snapshot) useCompanion.setState((state) => ({ snapshots: { ...state.snapshots, [session]: withoutPane(snapshot, pane) } }))
}

/**
 * Real use keeps the login's idle timer alive (the browser's automatic pongs
 * do not): at most one `session.ping` a minute, only after a tap or key.
 */
export function noteActivity(): void {
  const now = Date.now()
  if (now - lastPing < 60_000 || useCompanion.getState().socket !== "ready") return
  lastPing = now
  call<{ serverTime: string }>("session.ping").then(
    (result) => useCompanion.setState({ offset: clockOffset(result.serverTime) }),
    () => undefined,
  )
}

/** Clears the login locally; the cookie dies with the gateway session. */
export function endSession(reason: "expired" | "logged-out"): void {
  disconnect()
  storage.setBinding(null)
  useCompanion.setState({ hello: null, sessions: null, snapshots: {}, sessionErrors: {}, elevation: null })
  navigate({ name: "login", reason })
}

export async function logout(): Promise<void> {
  const binding = storage.binding()
  if (binding) await api.logout(binding).catch(() => undefined)
  endSession("logged-out")
}
