import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { ConnectedHost } from "@/lib/hostIpc"
import { useHostStore } from "@/state/hostStore"
import type { HostConfig } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"
import { HostConnectionsBridge } from "./HostConnectionsBridge"

const ipc = vi.hoisted(() => ({
  check: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), prepare: vi.fn(), request: vi.fn(),
  register: vi.fn(), unregister: vi.fn(),
}))
vi.mock("@/lib/hostIpc", () => ({
  checkHostRuntime: ipc.check, connectHost: ipc.connect, disconnectHost: ipc.disconnect,
  prepareHost: ipc.prepare, requestHost: ipc.request,
}))
vi.mock("@/lib/herdrProvider", () => ({ registerRuntimeHost: ipc.register, unregisterRuntimeHost: ipc.unregister }))
vi.mock("@/lib/remoteFiles", () => ({ reconnectRemoteWorkspaces: vi.fn(async () => undefined) }))

const originalHost = useHostStore.getState()
const originalSsh = useSshStore.getState()
const originalPreferences = useRuntimePreferencesStore.getState()
const measurement = import.meta.env.VITE_YUZORA_PERF_MEASURE === "1"

beforeEach(() => {
  vi.useFakeTimers()
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  vi.clearAllMocks()
  ipc.check.mockResolvedValue({ artifactIdentity: "a".repeat(64), check: { canApply: true } })
  ipc.request.mockResolvedValue({})
  ipc.disconnect.mockResolvedValue(undefined)
})
afterEach(() => {
  cleanup()
  useHostStore.setState(originalHost, true)
  useSshStore.setState(originalSsh, true)
  useRuntimePreferencesStore.setState(originalPreferences, true)
  vi.restoreAllMocks()
  vi.useRealTimers()
})

function seed(count: number) {
  const configs: Record<string, HostConfig> = {}
  const hosts: typeof originalHost.hosts = {}
  const sessions: typeof originalSsh.sessions = {}
  const descriptors: typeof originalSsh.hosts = []
  for (let index = 0; index < count; index++) {
    const id = `selection-fixture-${index}`
    const connection: ConnectedHost = {
      owner: { hostId: id, generation: index + 1 },
      hello: { protocol: 1, version: "fixture", os: "linux", arch: "x86_64", home: "/fixture", methods: [] },
    }
    configs[id] = { hostId: id, label: id, kind: "ssh", helper: "/fixture/helper", binary: "/fixture/herdr", artifactIdentity: "a".repeat(64) }
    hosts[id] = { connection, connecting: false, error: null, target: { kind: "ssh", sessionId: `ssh-${index}` }, attempt: 0, retryAt: 0 }
    sessions[id] = { hostId: id, sessionId: `ssh-${index}`, status: "connected", fingerprint: null, knownHost: true, error: null }
    descriptors.push({ id, name: id, host: "fixture.invalid", port: 22, user: "fixture", authKind: "password" })
  }
  descriptors.push({ id: "password-fixture", name: "Password", host: "fixture.invalid", port: 22, user: "fixture", authKind: "password" })
  useHostStore.setState({ ...originalHost, configs, hosts }, true)
  useSshStore.setState({ ...originalSsh, hosts: descriptors, sessions, activeHostId: descriptors[0].id }, true)
  return hosts
}

async function settle(action: () => void = () => undefined) {
  await act(async () => {
    action()
    // The fixture resolves host IPC immediately. Drain its existing promise chain;
    // do not advance the four-second health interval during user actions.
    for (let index = 0; index < 8; index++) await Promise.resolve()
  })
}

describe("helper checks during SSH view changes", () => {
  it("keeps selection and password prompts local while periodic health checks still run", async () => {
    seed(2)
    render(<HostConnectionsBridge />)
    await settle()
    const before = ipc.request.mock.calls.length
    await settle(() => useSshStore.getState().beginConnect("selection-fixture-1"))
    expect(useSshStore.getState().activeHostId).toBe("selection-fixture-1")
    await settle(() => useSshStore.getState().beginConnect("password-fixture"))
    expect(useSshStore.getState().pendingAuthHostId).toBe("password-fixture")
    await settle(() => useSshStore.getState().cancelPendingAuth())
    await settle(() => useRuntimePreferencesStore.setState({ wslEnabled: useRuntimePreferencesStore.getState().wslEnabled }))
    expect(useSshStore.getState().pendingAuthHostId).toBeNull()
    expect(ipc.request).toHaveBeenCalledTimes(before)
    await act(async () => vi.advanceTimersByTimeAsync(4000))
    expect(ipc.request).toHaveBeenCalledTimes(before + 2)
  })

  it("immediately reconciles host renames and disconnected sessions", async () => {
    const connections = seed(2)
    render(<HostConnectionsBridge />)
    await settle()
    const id = "selection-fixture-0"
    const descriptor = useSshStore.getState().hosts.find(host => host.id === id)!
    await settle(() => useSshStore.getState().updateHost(id, { ...descriptor, name: "Renamed fixture" }))
    expect(useHostStore.getState().configs[id].label).toBe("Renamed fixture")
    expect(ipc.register).toHaveBeenCalledWith(connections[id].connection, "/fixture/herdr", "Renamed fixture", "ssh")
    expect(ipc.disconnect).not.toHaveBeenCalled()
    // Model the native SSH connection event entering the real store. The helper
    // must close immediately, without advancing a health-check interval.
    await settle(() => useSshStore.setState(state => ({ sessions: {
      ...state.sessions, [id]: { ...state.sessions[id], sessionId: null, status: "disconnected" },
    } })))
    expect(ipc.disconnect).toHaveBeenCalledExactlyOnceWith(connections[id].connection!.owner)
    expect(useHostStore.getState().hosts[id].connection).toBeNull()
    expect(useHostStore.getState().hosts["selection-fixture-1"].connection).toBe(connections["selection-fixture-1"].connection)
    expect(ipc.connect).not.toHaveBeenCalled()
  })

  it.each([1, 8, 32])("measures actual store reconciliation for %i healthy helpers", async count => {
    const connections = seed(count)
    const view = render(<HostConnectionsBridge />)
    await settle()
    expect(ipc.request).toHaveBeenCalledTimes(count)
    const results = []
    const actions = {
      selection: (index: number) => useSshStore.getState().beginConnect(`selection-fixture-${index % count}`),
      password: () => {
        useSshStore.getState().beginConnect("password-fixture")
        useSshStore.getState().cancelPendingAuth()
      },
    }
    for (const [scenario, action] of Object.entries(actions)) {
      for (let index = 0; index < 10; index++) await settle(() => action(index))
      const before = ipc.request.mock.calls.length
      let notifications = 0
      const stop = useHostStore.subscribe(() => { notifications++ })
      for (let index = 0; index < 100; index++) await settle(() => action(index))
      stop()
      results.push({ scenario, actions: 100, requests: ipc.request.mock.calls.length - before, notifications })
      expect(useSshStore.getState().pendingAuthHostId).toBeNull()
      for (const [id, status] of Object.entries(connections)) expect(useHostStore.getState().hosts[id].connection).toBe(status.connection)
    }
    expect(ipc.connect).not.toHaveBeenCalled()
    expect(ipc.disconnect).not.toHaveBeenCalled()
    for (const [, request] of ipc.request.mock.calls) expect(request).toEqual({ method: "hello" })
    view.unmount()
    expect(vi.getTimerCount()).toBe(0)
    if (measurement) console.log("HOST_SELECTION_METRICS", JSON.stringify({ count, results, remainingTimers: vi.getTimerCount() }))
  })

  it("releases timers and subscriptions across repeated mount and unmount", async () => {
    seed(8)
    const subscriptions = new Set<() => void>()
    const listeners = new Set<EventListenerOrEventListenerObject>()
    for (const store of [useSshStore, useRuntimePreferencesStore]) {
      const subscribe = store.subscribe
      vi.spyOn(store, "subscribe").mockImplementation((listener: Parameters<typeof subscribe>[0]) => {
        // Both stores expose the same subscribe/unsubscribe lifecycle; keep the
        // original implementation so measurements observe real notifications.
        const release = subscribe(listener as never)
        const tracked = () => { release(); subscriptions.delete(tracked) }
        subscriptions.add(tracked)
        return tracked
      })
    }
    const add = document.addEventListener.bind(document)
    const remove = document.removeEventListener.bind(document)
    vi.spyOn(document, "addEventListener").mockImplementation((type, listener, options) => {
      if (type === "visibilitychange") listeners.add(listener)
      add(type, listener, options)
    })
    vi.spyOn(document, "removeEventListener").mockImplementation((type, listener, options) => {
      if (type === "visibilitychange") listeners.delete(listener)
      remove(type, listener, options)
    })
    let measuredRequests = 0
    for (let cycle = 0; cycle < 110; cycle++) {
      const before = ipc.request.mock.calls.length
      const view = render(<HostConnectionsBridge />)
      await settle()
      await settle(() => useSshStore.getState().setActiveHost(`selection-fixture-${cycle % 8}`))
      await act(async () => vi.advanceTimersByTimeAsync(4000))
      view.unmount()
      if (cycle >= 10) measuredRequests += ipc.request.mock.calls.length - before
      expect(subscriptions.size).toBe(0)
      expect(listeners.size).toBe(0)
      expect(vi.getTimerCount()).toBe(0)
      const after = ipc.request.mock.calls.length
      await settle(() => useSshStore.getState().setActiveHost("selection-fixture-0"))
      expect(ipc.request).toHaveBeenCalledTimes(after)
    }
    if (measurement) console.log("HOST_SELECTION_LIFECYCLE", JSON.stringify({ warmup: 10, cycles: 100, helpers: 8, measuredRequests, subscriptions: subscriptions.size, listeners: listeners.size, timers: vi.getTimerCount() }))
  })
})
