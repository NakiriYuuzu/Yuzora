import { beforeEach, expect, it, vi } from "vitest"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { useCompanionStore } from "./companionStore"

let replies: CompanionReplies
const store = () => useCompanionStore.getState()

beforeEach(() => {
  replies = { companion_status: fx.overviewRunning, companion_devices: [fx.device] }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ overview: null, devices: [], peers: {}, streams: {}, streamSeq: {}, lastGitRead: {}, claims: [], settled: {}, enableError: null })
})

const control = (stream: string, seq: number, open: boolean, deviceId = fx.device.deviceId) =>
  ({ type: "control.state", deviceId, peerId: 7, stream, seq, session: "default", target: "w1:p1", takeover: false, open }) as const
const full = (stream: string, seq: number, open: boolean) =>
  ({ type: "full.state", deviceId: fx.device.deviceId, peerId: 8, stream, seq, session: "default", open }) as const

it("upserts and removes devices from events", () => {
  store().applyEvent({ type: "device.registered", device: fx.device })
  store().applyEvent({ type: "device.updated", device: { ...fx.device, label: "Work phone" } })
  expect(store().devices).toEqual([{ ...fx.device, label: "Work phone" }])
  store().applyEvent({ type: "device.revoked", deviceId: fx.device.deviceId })
  expect(store().devices).toEqual([])
})

it("queues claims and drops one once its pairing settles", () => {
  const claim = { pairingId: "p1", deviceLabel: "iPhone", sas: "123456", via: "tailscale", tailscaleLogin: "u@example.com", userAgent: "Safari" }
  store().applyEvent({ type: "pairing.claimed", ...claim })
  store().applyEvent({ type: "pairing.claimed", ...claim, pairingId: "p2" })
  expect(store().claims.map(item => item.pairingId)).toEqual(["p1", "p2"])
  expect(store().claims[0]).toEqual(claim)
  store().applyEvent({ type: "pairing.state", pairingId: "p1", state: "rejected", reason: "cancelled" })
  expect(store().claims.map(item => item.pairingId)).toEqual(["p2"])
  expect(store().settled).toEqual({ p1: "rejected" })
})

it("drops pending claims once the gateway stops, since none can be approved", () => {
  const claim = { pairingId: "p1", deviceLabel: "iPhone", sas: "123456", via: "tailscale", tailscaleLogin: "u@example.com", userAgent: "Safari" }
  store().applyEvent({ type: "pairing.claimed", ...claim })
  store().applyEvent({ type: "status", status: fx.statusStopping })
  expect(store().claims).toHaveLength(1)
  store().applyEvent({ type: "status", status: fx.statusFailed })
  expect(store().claims).toEqual([])
})

it("re-reads the overview once a gateway stops, since the enable deadline is gone", async () => {
  useCompanionStore.setState({ overview: fx.overviewRunning })
  replies.companion_status = fx.overviewOff
  store().applyEvent({ type: "status", status: { state: "disabled" } })
  expect(store().overview?.status).toEqual({ state: "disabled" })
  await vi.waitFor(() => expect(store().overview).toEqual(fx.overviewOff))
})

it("refreshes again when an event lands while it reads", async () => {
  let resolve: () => void = () => undefined
  let first = true
  replies.companion_status = () => first ? new Promise(done => { resolve = () => done(fx.overviewOff) }) : fx.overviewRunning
  const refreshing = store().refresh()
  first = false
  store().applyEvent({ type: "device.revoked", deviceId: "other" })
  resolve()
  await refreshing
  expect(store().overview).toEqual(fx.overviewRunning)
})

it("follows the connected phones from session events", () => {
  store().applyEvent({ type: "session.opened", peerId: 3, deviceId: fx.device.deviceId })
  store().applyEvent({ type: "session.opened", peerId: 4, deviceId: "other" })
  expect(store().peers).toEqual({ 3: fx.device.deviceId, 4: "other" })
  store().applyEvent({ type: "session.closed", peerId: 3, deviceId: fx.device.deviceId, reason: "closed" })
  expect(store().peers).toEqual({ 4: "other" })
})

it("tracks open control and full streams, and drops them when they close", () => {
  store().applyEvent(control("6", 1, true))
  store().applyEvent(full("9", 1, true))
  expect(store().streams).toEqual({
    6: { deviceId: fx.device.deviceId, peerId: 7, session: "default", target: "w1:p1", scope: "control" },
    9: { deviceId: fx.device.deviceId, peerId: 8, session: "default", scope: "full" },
  })
  store().applyEvent(control("6", 2, false))
  expect(Object.keys(store().streams)).toEqual(["9"])
  store().applyEvent(full("9", 2, false))
  expect(store().streams).toEqual({})
})

it("ignores a stream event that arrives after a newer one", () => {
  // The close overtook the open: the stream must not come back.
  store().applyEvent(control("6", 2, false))
  store().applyEvent(control("6", 1, true))
  expect(store().streams).toEqual({})
  // A repeated open is a no-op, not a second stream.
  store().applyEvent(control("7", 1, true))
  store().applyEvent(control("7", 1, true))
  expect(Object.keys(store().streams)).toEqual(["7"])
})

it("forgets every phone and stream once the gateway leaves running", () => {
  store().applyEvent({ type: "session.opened", peerId: 3, deviceId: fx.device.deviceId })
  store().applyEvent(control("6", 1, true))
  store().applyEvent({ type: "status", status: { state: "running", port: 47110 } })
  expect(Object.keys(store().peers)).toEqual(["3"])
  expect(Object.keys(store().streams)).toEqual(["6"])
  store().applyEvent({ type: "status", status: fx.statusStopping })
  expect(store().peers).toEqual({})
  expect(store().streams).toEqual({})
  // The next run starts its streams over at seq 1.
  store().applyEvent(control("6", 1, true))
  expect(Object.keys(store().streams)).toEqual(["6"])
})

it("takes the node and push state from events, and re-reads when one lands mid-refresh", async () => {
  useCompanionStore.setState({ overview: fx.overviewOff })
  store().applyEvent({ type: "net", net: fx.netNeedsLogin })
  expect(store().overview?.net).toEqual(fx.netNeedsLogin)
  store().applyEvent(fx.pushEvent)
  expect(store().overview?.push).toEqual(fx.pushOverview)
  expect(store().overview?.status).toEqual({ state: "disabled" })

  let resolve: () => void = () => undefined
  let first = true
  replies.companion_status = () => first ? new Promise(done => { resolve = () => done(fx.overviewOff) }) : { ...fx.overviewOff, net: fx.netRunning }
  const refreshing = store().refresh()
  first = false
  store().applyEvent({ type: "net", net: fx.netRunning })
  resolve()
  await refreshing
  expect(store().overview?.net).toEqual(fx.netRunning)
})

it("drops net and push events that arrive before the overview is read", () => {
  store().applyEvent({ type: "net", net: fx.netRunning })
  store().applyEvent({ type: "push", push: { available: true, subscribed: [] } })
  expect(store().overview).toBeNull()
})

const gitRead = (deviceId: string, at: string) =>
  ({ type: "audit", deviceId, peerId: 3, scope: "view", action: "git.read", session: "default", target: "w1", summary: { method: "git.log" }, at }) as const

it("remembers when each phone last read Git, from its audit, and nothing else", () => {
  store().applyEvent(gitRead(fx.device.deviceId, "2026-10-20T10:00:00.176376458Z"))
  store().applyEvent(gitRead("other", "2026-10-20T10:01:00Z"))
  expect(store().lastGitRead).toEqual({ [fx.device.deviceId]: "2026-10-20T10:00:00.176376458Z", other: "2026-10-20T10:01:00Z" })
  store().applyEvent(gitRead(fx.device.deviceId, "2026-10-20T10:05:00Z"))
  expect(store().lastGitRead[fx.device.deviceId]).toBe("2026-10-20T10:05:00Z")
  // Another audit says nothing about Git.
  store().applyEvent({ ...gitRead("third", "2026-10-20T10:06:00Z"), action: "agent.prompt", scope: "control" })
  expect(Object.keys(store().lastGitRead).sort()).toEqual([fx.device.deviceId, "other"].sort())
})

it("keeps the last enable failure until the Companion runs, whichever way that is learned", async () => {
  useCompanionStore.setState({ overview: fx.overviewOff })
  store().setEnableError("companion-net-https-disabled")
  expect(store().enableError).toBe("companion-net-https-disabled")
  // Winding down or failing is not running: the explanation stays.
  store().applyEvent({ type: "status", status: fx.statusStarting })
  store().setOverview({ ...fx.overviewOff, status: fx.statusFailed })
  expect(store().enableError).toBe("companion-net-https-disabled")
  store().applyEvent({ type: "status", status: fx.overviewRunning.status })
  expect(store().enableError).toBeNull()

  store().setEnableError("companion-net-https-disabled")
  store().setOverview(fx.overviewRunning)
  expect(store().enableError).toBeNull()

  store().setEnableError("companion-net-https-disabled")
  replies.companion_status = fx.overviewOff
  await store().refresh()
  expect(store().enableError).toBe("companion-net-https-disabled")
  replies.companion_status = fx.overviewRunning
  await store().refresh()
  expect(store().enableError).toBeNull()
})

it("never commits a read an event overtook, and reads again later", async () => {
  vi.useFakeTimers()
  try {
    useCompanionStore.setState({ overview: fx.overviewRunning, devices: [fx.device] })
    let calls = 0
    // Every early read still lists the device that the revoke removed.
    replies.companion_devices = () => {
      calls++
      if (calls <= 3) store().applyEvent({ type: "device.revoked", deviceId: fx.device.deviceId })
      return calls <= 3 ? [fx.device] : []
    }
    await store().refresh()
    expect(calls).toBe(3)
    expect(store().devices).toEqual([])
    await vi.advanceTimersByTimeAsync(1500)
    expect(calls).toBe(4)
    expect(store().devices).toEqual([])
  } finally {
    vi.useRealTimers()
  }
})

it("drops the streams of a phone whose session closed", () => {
  store().applyEvent(control("6", 1, true))
  store().applyEvent(full("9", 1, true))
  store().applyEvent({ type: "session.closed", peerId: 7, deviceId: fx.device.deviceId, reason: "closed" })
  expect(Object.keys(store().streams)).toEqual(["9"])
})
