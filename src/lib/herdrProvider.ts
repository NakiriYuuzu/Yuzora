import { Channel, invoke as nativeInvoke } from "@tauri-apps/api/core"
import type { ConnectedHost } from "./hostIpc"
import type { HerdrNamedSession, HerdrSubscriptionEvent, HerdrTerminalEvent } from "./herdrTypes"
import { LOCAL_HOST_ID, parseRemoteFilePath, runtimeKey, sameConnection } from "./runtimeIdentity"
import type { ConnectionOwner, RuntimeKey } from "./runtimeIdentity"
import { isWindowsPlatform } from "./platform"

interface RuntimeHost {
  owner: ConnectionOwner
  binary: string
  label: string
  sessions: HerdrNamedSession[]
}
const hosts = new Map<string, RuntimeHost>()
const streams = new Map<string, { host: RuntimeHost; streamId: string }>()

/** Legacy local session keys remain readable during the on-disk migration. */
export function sessionScope(session: HerdrNamedSession | null | undefined): string | null {
  return session ? session.runtimeId ?? session.name : null
}

/** Legacy `live` pages belong to the local runtime; never guess a remote host. */
export function findRuntimeSession(sessions: HerdrNamedSession[], scope: string): HerdrNamedSession | null {
  if (scope !== "live") return sessions.find((session) => sessionScope(session) === scope) ?? null
  const local = sessions.filter((session) => !session.hostId || session.hostId === LOCAL_HOST_ID)
  return local.find((session) => session.default) ?? local[0] ?? null
}

export function parseRuntimeScope(scope: string): RuntimeKey {
  if (!scope.startsWith("[")) return { hostId: LOCAL_HOST_ID, sessionName: scope }
  const parsed: unknown = JSON.parse(scope)
  if (!Array.isArray(parsed) || parsed.length !== 2 || parsed.some((part) => typeof part !== "string" || !part)) throw new Error("Invalid runtime identity")
  const key = { hostId: parsed[0] as string, sessionName: parsed[1] as string }
  if (runtimeKey(key) !== scope) throw new Error("Invalid runtime identity")
  return key
}

export function registerRuntimeHost(host: ConnectedHost, binary: string, label: string): void {
  if (host.owner.hostId === LOCAL_HOST_ID) throw new Error("Local runtime identity is reserved")
  const previous = hosts.get(host.owner.hostId)
  hosts.set(host.owner.hostId, { owner: host.owner, binary, label, sessions: previous?.sessions ?? [] })
}

export function unregisterRuntimeHost(owner: ConnectionOwner): void {
  const current = hosts.get(owner.hostId)
  if (current && sameConnection(current.owner, owner)) hosts.delete(owner.hostId)
}

export function runtimeOwner(scope: string): ConnectionOwner | null {
  const key = parseRuntimeScope(scope)
  return key.hostId === LOCAL_HOST_ID ? null : hosts.get(key.hostId)?.owner ?? null
}

export async function canonicalRuntimeWorkspace(scope: string, path: string): Promise<string> {
  const key = parseRuntimeScope(scope)
  const remote = parseRemoteFilePath(path)
  if (key.hostId === LOCAL_HOST_ID) {
    if (remote) throw new Error("Workspace belongs to a different runtime host")
    return nativeInvoke("workspace_canonical_path", { path })
  }
  if (remote && remote.hostId !== key.hostId) throw new Error("Workspace belongs to a different runtime host")
  const host = hosts.get(key.hostId)
  if (!host) throw new Error("Runtime host is disconnected")
  const { registerRuntimeWorkspace } = await import("./remoteFiles")
  const uri = await registerRuntimeWorkspace(host.owner, remote?.path ?? path, () => hosts.get(key.hostId) === host)
  ensureCurrent(host)
  return uri
}

export class StaleRuntimeResponse extends Error {
  constructor() { super("Runtime connection changed; response discarded") }
}

function ensureCurrent(host: RuntimeHost): void {
  if (hosts.get(host.owner.hostId) !== host) throw new StaleRuntimeResponse()
}

async function hostCall<T>(host: RuntimeHost, command: string, args?: Record<string, unknown>): Promise<T> {
  ensureCurrent(host)
  try {
    const value = await nativeInvoke<T>("host_request", {
      owner: host.owner,
      operation: { method: "herdrCall", params: { binary: host.binary, call: args ? { command, args } : { command } } }
    })
    ensureCurrent(host)
    return value
  } catch (error) {
    ensureCurrent(host)
    throw error
  }
}

type StreamEvent =
  | { type: "frame"; streamId: string; frame: { version: number; owner: ConnectionOwner; payload: { type: "terminal"; event: HerdrTerminalEvent } | { type: "subscription"; event: HerdrSubscriptionEvent } } }
  | { type: "closed"; streamId: string; owner: ConnectionOwner; reason: string }

async function openStream<T>(host: RuntimeHost, sessionName: string, command: string, args: Record<string, unknown>): Promise<T> {
  const terminal = command === "herdr_terminal_open"
  const output = args.onEvent as Channel<HerdrTerminalEvent | HerdrSubscriptionEvent>
  const channel = new Channel<StreamEvent>()
  let closed = false
  const identity = (id: string) => JSON.stringify([host.owner.hostId, host.owner.generation, id])
  channel.onmessage = (message) => {
    if (hosts.get(host.owner.hostId) !== host) return
    const owner = message.type === "frame" ? message.frame.owner : message.owner
    if (!sameConnection(owner, host.owner)) return
    const id = identity(message.streamId)
    if (message.type === "closed") {
      closed = true
      streams.delete(id)
      output.onmessage(terminal
        ? { type: "error", sessionId: id, code: "host-stream-closed", message: message.reason }
        : { type: "disconnected", subscriptionId: id, reason: message.reason })
      return
    }
    if (message.frame.version !== 1) return
    const payload = message.frame.payload
    if (payload.type === "terminal") output.onmessage({ ...payload.event, sessionId: id })
    if (payload.type === "subscription") output.onmessage({ ...payload.event, subscriptionId: id })
  }
  const opened = await nativeInvoke<{ streamId: string; value: Record<string, unknown> | string }>("host_stream_open", {
    owner: host.owner,
    config: terminal
      ? { kind: "terminal", binary: host.binary, sessionName, target: args.target, mode: args.mode ?? "observe", takeover: args.takeover ?? false, cols: args.cols, rows: args.rows }
      : { kind: "events", binary: host.binary, sessionName, paneIds: args.paneIds ?? [] },
    onEvent: channel
  })
  if (hosts.get(host.owner.hostId) !== host) {
    await nativeInvoke("host_stream_close", { owner: host.owner, streamId: opened.streamId }).catch(() => undefined)
    throw new StaleRuntimeResponse()
  }
  const id = identity(opened.streamId)
  if (closed) throw new Error("Remote connector closed during opening")
  streams.set(id, { host, streamId: opened.streamId })
  return (terminal ? { ...(opened.value as object), sessionId: id } : id) as T
}

/** Central routing boundary shared by every typed HERDR wrapper. */
export async function invokeHerdr<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (command === "herdr_sessions") {
    const remote = [...hosts.values()]
    const results = await Promise.allSettled([
      isWindowsPlatform() ? Promise.resolve([] as HerdrNamedSession[]) : nativeInvoke<HerdrNamedSession[]>(command),
      ...remote.map(async (host) => {
        const sessions = await hostCall<HerdrNamedSession[]>(host, command)
        host.sessions = sessions.map((session) => ({ ...session, hostId: host.owner.hostId, hostLabel: host.label, runtimeId: runtimeKey({ hostId: host.owner.hostId, sessionName: session.name }) }))
        return host.sessions
      })
    ])
    const sessions = results.flatMap((result, index) => result.status === "fulfilled"
      ? result.value
      : index > 0 && hosts.get(remote[index - 1].owner.hostId) === remote[index - 1]
        ? ["host-call-limit", "host-call-wait-timeout", "host-request-limit", "host-request-wait-timeout"].includes(result.reason instanceof Error ? result.reason.message : String(result.reason))
          ? remote[index - 1].sessions
          : remote[index - 1].sessions.map((session) => ({ ...session, running: false })) : [])
    if (results[0].status === "rejected" && remote.length === 0) throw results[0].reason
    return sessions as T
  }
  const resourceId = args.sessionId ?? args.subscriptionId
  if (typeof resourceId === "string" && resourceId.startsWith("[")) {
    const stream = streams.get(resourceId)
    if (!stream) {
      if (command.endsWith("_release")) return undefined as T
      throw new Error("Remote connector is closed")
    }
    ensureCurrent(stream.host)
    if (command.endsWith("_release")) {
      streams.delete(resourceId)
      return nativeInvoke<T>("host_stream_close", { owner: stream.host.owner, streamId: stream.streamId })
    }
    const operation = command === "herdr_terminal_input" ? { command: "input", text: args.text, bytesBase64: args.bytesBase64 }
      : command === "herdr_terminal_resize" ? { command: "resize", cols: args.cols, rows: args.rows }
        : command === "herdr_terminal_scroll" ? { command: "scroll", direction: args.direction, lines: args.lines } : null
    if (!operation) throw new Error("Unsupported stream command")
    return nativeInvoke<T>("host_stream_command", { owner: stream.host.owner, streamId: stream.streamId, operation })
  }
  const scope = typeof args.sessionName === "string" ? parseRuntimeScope(args.sessionName) : null
  if (!scope || scope.hostId === LOCAL_HOST_ID) return nativeInvoke<T>(command, { ...args, ...(scope ? { sessionName: scope.sessionName } : {}) })
  const host = hosts.get(scope.hostId)
  if (!host) throw new Error("Runtime host is disconnected")
  const routed = { ...args, sessionName: scope.sessionName }
  if (typeof args.cwd === "string") {
    const remote = parseRemoteFilePath(args.cwd)
    if (remote && remote.hostId !== scope.hostId) throw new Error("Workspace belongs to a different runtime host")
    if (remote) Object.assign(routed, { cwd: remote.path })
  }
  if (command === "herdr_terminal_open" || command === "herdr_events_subscribe") return openStream<T>(host, scope.sessionName, command, routed)
  return hostCall<T>(host, command, routed)
}
