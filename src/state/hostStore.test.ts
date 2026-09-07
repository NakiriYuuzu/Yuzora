import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { ConnectedHost } from "@/lib/hostIpc"

const mocks = vi.hoisted(() => ({
  prepare: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), request: vi.fn(), register: vi.fn(), unregister: vi.fn(),
  ssh: { sessions: {} as Record<string, { sessionId: string; status: string }>, hosts: [] as Array<{ id: string; name: string }> }
}))
vi.mock("@/lib/hostIpc", () => ({ prepareHost: mocks.prepare, connectHost: mocks.connect, disconnectHost: mocks.disconnect, requestHost: mocks.request }))
vi.mock("@/lib/herdrProvider", () => ({ registerRuntimeHost: mocks.register, unregisterRuntimeHost: mocks.unregister }))
vi.mock("./sshStore", () => ({ useSshStore: { getState: () => mocks.ssh } }))

const host = (generation = 1): ConnectedHost => ({ owner: { hostId: "host", generation }, hello: { protocol: 1, version: "test", os: "linux", arch: "x86_64", home: "/home/test", methods: [] } })
const config = { hostId: "host", label: "Server", kind: "ssh" as const, helper: "/helper", binary: "/herdr" }
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void; const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail }); return { promise, resolve, reject } }
beforeEach(() => {
  vi.resetModules()
  vi.clearAllMocks()
  const values = new Map<string, string>()
  vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value) } })
  mocks.ssh.sessions = { host: { sessionId: "ssh-1", status: "connected" } }
  mocks.ssh.hosts = []
  mocks.disconnect.mockResolvedValue(undefined)
  mocks.request.mockResolvedValue({})
})
afterEach(() => vi.unstubAllGlobals())

it("disconnect during setup discards and releases the late helper", async () => {
  const { useHostStore } = await import("./hostStore")
  const pending = deferred<{ connection: ConnectedHost; helper: string; binary: string }>()
  mocks.prepare.mockReturnValue(pending.promise)
  const setup = useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })
  const assertion = expect(setup).rejects.toThrow("cancelled")
  await vi.waitFor(() => expect(mocks.prepare).toHaveBeenCalledOnce())
  await useHostStore.getState().disconnect("host")
  pending.resolve({ connection: host(), helper: "/helper", binary: "/herdr" })
  await assertion
  expect(mocks.register).not.toHaveBeenCalled()
  expect(mocks.disconnect).toHaveBeenCalledWith(host().owner)
  expect(useHostStore.getState().hosts.host).toMatchObject({ connection: null, connecting: false })
})

it("does not publish a reconnect after the SSH identity changes", async () => {
  const { useHostStore } = await import("./hostStore")
  useHostStore.setState({ configs: { host: config } })
  const pending = deferred<ConnectedHost>()
  mocks.connect.mockReturnValue(pending.promise)
  useHostStore.getState().reconcile()
  await vi.waitFor(() => expect(mocks.connect).toHaveBeenCalledOnce())
  mocks.ssh.sessions.host = { sessionId: "ssh-new", status: "connected" }
  pending.resolve(host())
  await vi.waitFor(() => expect(mocks.disconnect).toHaveBeenCalledWith(host().owner))
  expect(mocks.register).not.toHaveBeenCalled()
})

it("keeps an explicitly disconnected host paused across reconcile", async () => {
  const { useHostStore } = await import("./hostStore")
  useHostStore.setState({ configs: { host: config }, hosts: { host: { connection: host(), connecting: false, error: null, target: { kind: "ssh", sessionId: "ssh-1" }, attempt: 0, retryAt: 0 } } })
  await useHostStore.getState().disconnect("host")
  useHostStore.getState().reconcile()
  expect(mocks.connect).not.toHaveBeenCalled()
  expect(mocks.request).not.toHaveBeenCalled()
  expect(mocks.unregister).toHaveBeenCalledWith(host().owner)
})

it.each(["host-request-limit", "host-request-wait-timeout"])("preserves the host generation on a busy health check: %s", async (error) => {
  const { useHostStore } = await import("./hostStore")
  const connected = host()
  useHostStore.setState({ configs: { host: config }, hosts: { host: { connection: connected, connecting: false, error: null, target: { kind: "ssh", sessionId: "ssh-1" }, attempt: 0, retryAt: 0 } } })
  mocks.request.mockRejectedValueOnce(error)
  useHostStore.getState().reconcile()
  await vi.waitFor(() => expect(mocks.request).toHaveBeenCalledOnce())
  expect(mocks.disconnect).not.toHaveBeenCalled()
  expect(mocks.unregister).not.toHaveBeenCalled()
  expect(useHostStore.getState().hosts.host.connection).toBe(connected)
  useHostStore.getState().reconcile()
  await vi.waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(2))
})

it("updates a display name without reconnecting or changing runtime identity", async () => {
  const { useHostStore } = await import("./hostStore")
  const connected = host()
  useHostStore.setState({ configs: { host: config }, hosts: { host: { connection: connected, connecting: false, error: null, target: { kind: "ssh", sessionId: "ssh-1" }, attempt: 0, retryAt: 0 } } })
  mocks.ssh.hosts = [{ id: "host", name: "Renamed" }]
  useHostStore.getState().reconcile()
  expect(useHostStore.getState().configs.host.label).toBe("Renamed")
  expect(mocks.register).toHaveBeenCalledWith(connected, "/herdr", "Renamed")
  expect(mocks.connect).not.toHaveBeenCalled()
  expect(mocks.disconnect).not.toHaveBeenCalled()
})

it("releases a prepared connection when persistence fails", async () => {
  const { useHostStore } = await import("./hostStore")
  vi.stubGlobal("localStorage", { setItem: () => { throw new Error("disk full") } })
  mocks.prepare.mockResolvedValue({ connection: host(), helper: "/helper", binary: "/herdr" })
  await expect(useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })).rejects.toThrow("disk full")
  expect(mocks.register).not.toHaveBeenCalled()
  expect(mocks.disconnect).toHaveBeenCalledWith(host().owner)
  expect(useHostStore.getState().configs.host).toBeUndefined()
})

it.each([false, true])("queues one explicit setup behind a health check (failure: %s)", async (failed) => {
  const { useHostStore } = await import("./hostStore")
  useHostStore.setState({ configs: { host: config }, hosts: { host: { connection: host(), connecting: false, error: null, target: { kind: "ssh", sessionId: "ssh-1" }, attempt: 0, retryAt: 0 } } })
  const health = deferred<unknown>()
  mocks.request.mockReturnValueOnce(health.promise)
  mocks.prepare.mockResolvedValue({ connection: host(2), helper: "/updated-helper", binary: "/herdr" })
  useHostStore.getState().reconcile()
  const setup = useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })
  await expect(useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })).rejects.toThrow("already in progress")
  useHostStore.getState().reconcile()
  expect(mocks.request).toHaveBeenCalledOnce()
  expect(mocks.prepare).not.toHaveBeenCalled()
  expect(mocks.disconnect).not.toHaveBeenCalled()
  if (failed) health.reject(new Error("connection lost"))
  else health.resolve({})
  await expect(setup).resolves.toEqual(host(2))
  expect(mocks.prepare).toHaveBeenCalledOnce()
  expect(mocks.disconnect).toHaveBeenCalledExactlyOnceWith(host().owner)
  expect(useHostStore.getState().hosts.host).toMatchObject({ connection: host(2), error: null })
})

it("releases a late background reconnect before preparing updated tools", async () => {
  const { useHostStore } = await import("./hostStore")
  useHostStore.setState({ configs: { host: config } })
  const reconnect = deferred<ConnectedHost>()
  const release = deferred<void>()
  mocks.connect.mockReturnValueOnce(reconnect.promise)
  mocks.disconnect.mockReturnValueOnce(release.promise)
  mocks.prepare.mockResolvedValue({ connection: host(2), helper: "/updated-helper", binary: "/herdr" })
  useHostStore.getState().reconcile()
  await vi.waitFor(() => expect(mocks.connect).toHaveBeenCalledOnce())
  const setup = useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })
  reconnect.resolve(host())
  await vi.waitFor(() => expect(mocks.disconnect).toHaveBeenCalledWith(host().owner))
  expect(mocks.register).not.toHaveBeenCalled()
  expect(mocks.prepare).not.toHaveBeenCalled()
  release.resolve()
  await expect(setup).resolves.toEqual(host(2))
  expect(mocks.register).toHaveBeenCalledExactlyOnceWith(host(2), "/herdr", "Server")
})

it("cancels a queued setup without deploying or reviving the host", async () => {
  const { useHostStore } = await import("./hostStore")
  useHostStore.setState({ configs: { host: config }, hosts: { host: { connection: host(), connecting: false, error: null, target: { kind: "ssh", sessionId: "ssh-1" }, attempt: 0, retryAt: 0 } } })
  const health = deferred<unknown>()
  mocks.request.mockReturnValueOnce(health.promise)
  useHostStore.getState().reconcile()
  const setup = useHostStore.getState().setup("host", "Server", { kind: "ssh", sessionId: "ssh-1" })
  const assertion = expect(setup).rejects.toThrow("cancelled")
  await useHostStore.getState().disconnect("host")
  health.resolve({})
  await assertion
  expect(mocks.prepare).not.toHaveBeenCalled()
  expect(useHostStore.getState().hosts.host.connection).toBeNull()
  useHostStore.getState().reconcile()
  expect(mocks.connect).not.toHaveBeenCalled()
})
