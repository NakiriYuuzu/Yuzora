import { withAgentStatus, withoutPane } from "./lib/agents"
import { api } from "./lib/api"
import type { PaneTarget } from "./lib/deepLink"
import type { AgentStatus, Hello, SessionInfo, Snapshot, StreamOpen } from "./lib/protocol"
import { CompanionSocket, RpcError } from "./lib/rpc"
import { clockOffset } from "./lib/time"
import { navigate, open, storage, useCompanion } from "./store"

type StreamHandlers = { data: (bytes: Uint8Array) => void; closed: (reason: string) => void }

const streams = new Map<number, StreamHandlers>()
// Frames that arrive before their stream's listener is registered.
const early = new Map<number, Uint8Array[]>()
const EARLY_FRAMES = 64
// Streams this page stopped listening to; their late frames are dropped.
const ended = new Set<number>()
// Closures that arrived before their stream's listener (a control stream can
// report `owned-elsewhere` right after it opens), replayed on `listen`.
const closedEarly = new Map<number, string>()
const EARLY_CLOSURES = 16
// Each session's event subscription, and its pending re-subscription.
const eventStreams = new Map<string, number>()
const reloads = new Map<string, ReturnType<typeof setTimeout>>()
const lastReload = new Map<string, number>()
// Events seen per session: a snapshot read across one is read again.
const touched = new Map<string, number>()
const SNAPSHOT_RETRIES = 2
let socket: CompanionSocket | null = null
let lastPing = 0

function socketUrl(): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`
}

/** Connects with the stored binding; a 4401 sends the user back to login. */
export function connect(): void {
  socket?.stop()
  streams.clear()
  closedEarly.clear()
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
        closedEarly.clear()
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
        const watched = [...eventStreams.values()].includes(stream)
        if (!handlers && !watched && !ended.has(stream)) {
          // Nobody listens yet: keep the closure (and any early frames) for `listen`.
          if (closedEarly.size >= EARLY_CLOSURES) {
            // The evicted closure's frames go with it: nobody can replay them now.
            const oldest = closedEarly.keys().next().value!
            closedEarly.delete(oldest)
            early.delete(oldest)
          }
          closedEarly.set(stream, String(data.reason))
        } else early.delete(stream)
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
  closedEarly.clear()
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
  const reason = closedEarly.get(stream)
  if (reason !== undefined) {
    closedEarly.delete(stream)
    for (const bytes of early.get(stream) ?? []) handlers.data(bytes)
    early.delete(stream)
    handlers.closed(reason)
    return () => undefined
  }
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
  const listed = (name: string) => sessions.some((item) => item.name === name)
  // A session that is gone keeps neither its subscription nor its error.
  for (const [name, stream] of eventStreams) {
    if (listed(name)) continue
    eventStreams.delete(name)
    void call("stream.close", { stream }).catch(() => undefined)
  }
  useCompanion.setState((state) => ({
    sessions,
    snapshots: Object.fromEntries(Object.entries(state.snapshots).filter(([name]) => listed(name))),
    sessionErrors: Object.fromEntries(Object.entries(state.sessionErrors).filter(([name]) => listed(name))),
  }))
  const loaded = await Promise.all(sessions.map(async (session) => [session.name, await loadSession(session.name, false)] as const))
  openPendingPane(new Set(loaded.filter(([, ok]) => !ok).map(([name]) => name)))
}

/**
 * Takes the user to the pane a notification pointed at. A pane the snapshots
 * don't show yet waits for the next refresh (started now when connected); one
 * that is gone then is dropped.
 */
export function showPane(target: PaneTarget): void {
  useCompanion.setState({ pendingPane: target })
  if (paneKnown(target)) openPendingPane()
  else if (useCompanion.getState().socket === "ready") void refresh()
}

const paneKnown = ({ session, pane }: PaneTarget) => useCompanion.getState().snapshots[session]?.panes.some((item) => item.id === pane) ?? false

function openPendingPane(failed: ReadonlySet<string> = new Set()): void {
  const { pendingPane: target, screen } = useCompanion.getState()
  if (!target) return
  // Its session's snapshot did not load, so the pane is not known to be gone.
  if (!paneKnown(target) && failed.has(target.session)) return
  useCompanion.setState({ pendingPane: null })
  const showing = screen.name === "pane" && screen.session === target.session && screen.pane === target.pane
  if (paneKnown(target) && !showing) open({ name: "pane", session: target.session, pane: target.pane })
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

/** Whether the session's snapshot was committed. */
async function loadSession(name: string, resubscribe: boolean): Promise<boolean> {
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
    let snapshot: Snapshot
    let raced: boolean
    for (let retry = 0; ; retry++) {
      const mark = touched.get(name)
      snapshot = await call<Snapshot>("snapshot.get", { session: name })
      // An event that landed during the read may be older or newer than it.
      raced = touched.get(name) !== mark
      if (!raced || retry === SNAPSHOT_RETRIES) break
    }
    useCompanion.setState((state) => ({ snapshots: { ...state.snapshots, [name]: snapshot }, sessionErrors: withoutKey(state.sessionErrors, name) }))
    if (raced) scheduleReload(name)
    return true
  } catch (error) {
    if (error instanceof RpcError && error.code !== "disconnected") {
      useCompanion.setState((state) => ({ sessionErrors: { ...state.sessionErrors, [name]: error.code } }))
    }
    return false
  }
}

function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record }
  delete next[key]
  return next
}

const touch = (session: string) => touched.set(session, (touched.get(session) ?? 0) + 1)

function applyStatus(session: string, pane: string, status: AgentStatus): void {
  touch(session)
  const snapshot = useCompanion.getState().snapshots[session]
  if (!snapshot) return
  const next = withAgentStatus(snapshot, pane, status)
  if (next) useCompanion.setState((state) => ({ snapshots: { ...state.snapshots, [session]: next } }))
  // A pane that became an agent after the snapshot.
  else scheduleReload(session)
}

function applyExit(session: string, pane: string): void {
  touch(session)
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
  useCompanion.setState({ hello: null, sessions: null, snapshots: {}, sessionErrors: {}, git: {}, spaceView: {}, elevation: null })
  navigate({ name: "login", reason })
}

// The gateway answers every logout with a Set-Cookie that clears the login
// cookie, so a late answer would also clear the cookie of a newer login.
const LOGOUT_DEADLINE_MS = 3000
let pendingLogout: AbortController | null = null

/** Drops an unanswered logout request; its answer must never reach the browser's cookie jar. */
export function abortLogout(): void {
  pendingLogout?.abort()
  pendingLogout = null
}

export async function logout(): Promise<void> {
  const binding = storage.binding()
  endSession("logged-out")
  // Best effort: a slow or unreachable gateway must not keep the login.
  if (!binding) return
  abortLogout()
  const request = new AbortController()
  pendingLogout = request
  const deadline = setTimeout(() => request.abort(), LOGOUT_DEADLINE_MS)
  void api.logout(binding, request.signal).catch(() => undefined).finally(() => {
    clearTimeout(deadline)
    if (pendingLogout === request) pendingLogout = null
  })
}
